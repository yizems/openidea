//! Live connections (one small pool per data source) and statement execution.
//!
//! Statements run through `raw_sql` (PostgreSQL's simple query protocol,
//! MySQL's text protocol), so every value arrives as text and is shown as is,
//! whatever its type.

use crate::source::{DataSource, DbDriver, DbSslMode};
use bb8::ManageConnection;
use bb8::Pool as Bb8Pool;
use futures::StreamExt;
use ide_core::{CancellationToken, IdeError, IdeResult, error::ErrorKind};
use odbc_api::{
    ConnectionOptions as OdbcConnectionOptions, Cursor as _, DataType as OdbcDataType, ResultSetMetadata as _,
    buffers::TextRowSet, escape_attribute_value as odbc_escape,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{
    Column, ColumnIndex, Connection, Decode, Executor, MySql, Postgres, Row, SqlSafeStr, Sqlite, TypeInfo, ValueRef,
    mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlSslMode},
    pool::PoolConnection,
    postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgSslMode},
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
#[serde(rename = "DbResultColumn")]
pub struct ResultColumn {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
#[serde(rename = "DbQueryResult")]
pub struct QueryResult {
    pub statement: String,
    pub columns: Vec<ResultColumn>,
    /// Cell text; `None` is SQL NULL. Binary values are `0x…` hex.
    pub rows: Vec<Vec<Option<String>>>,
    /// Rows changed by a DML statement (`None` for queries).
    pub affected: Option<f64>,
    /// More rows exist beyond the limit.
    pub has_more: bool,
    pub elapsed_ms: f64,
    /// Until the server's first answer; the rest of `elapsed_ms` is fetching.
    pub execution_ms: f64,
}

// bb8-tiberius never marks connections broken; keep an empty slot when discarded.
pub struct MssqlConnectionManager(bb8_tiberius::ConnectionManager);

pub struct MssqlConnection(Option<bb8_tiberius::rt::Client>);

impl std::ops::Deref for MssqlConnection {
    type Target = bb8_tiberius::rt::Client;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("live SQL Server connection")
    }
}

impl std::ops::DerefMut for MssqlConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("live SQL Server connection")
    }
}

impl ManageConnection for MssqlConnectionManager {
    type Connection = MssqlConnection;
    type Error = bb8_tiberius::Error;

    async fn connect(&self) -> Result<Self::Connection, Self::Error> {
        self.0.connect().await.map(|c| MssqlConnection(Some(c)))
    }

    async fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        conn.simple_query("SELECT 1").await?.into_results().await?;
        Ok(())
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        conn.0.is_none()
    }
}

pub enum Pool {
    Sqlite(SqlitePool),
    Postgres(PgPool),
    Mysql(MySqlPool),
    SqlServer(Bb8Pool<MssqlConnectionManager>),
    Oracle(Bb8Pool<OdbcConnectionManager>),
    Db2(Bb8Pool<OdbcConnectionManager>),
}

/// Runs `$body` with `$p` bound to the concrete pool of each driver.
macro_rules! with_pool {
    ($pool:expr, $p:ident => $body:expr) => {
        match $pool {
            Pool::Sqlite($p) => $body,
            Pool::Postgres($p) => $body,
            Pool::Mysql($p) => $body,
            Pool::SqlServer(_) | Pool::Oracle(_) | Pool::Db2(_) => unreachable!(),
        }
    };
}

pub(crate) fn db_err(e: sqlx::Error) -> IdeError {
    match &e {
        sqlx::Error::Database(d) => {
            let code = d.code().map(|c| c.into_owned());
            // Wrong password: PostgreSQL 28P01, MySQL 1045 (SQLSTATE 28000).
            let auth = matches!(code.as_deref(), Some("28P01" | "28000" | "1045"));
            let msg = match code {
                Some(c) => format!("[{c}] {}", d.message()),
                None => d.message().to_owned(),
            };
            IdeError::new(if auth { ErrorKind::PermissionDenied } else { ErrorKind::InvalidInput }, msg)
        }
        sqlx::Error::PoolTimedOut => IdeError::new(ErrorKind::Io, "timed out connecting to the database"),
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => IdeError::new(ErrorKind::Io, e.to_string()),
        _ => IdeError::new(ErrorKind::Protocol, e.to_string()),
    }
}

pub(crate) fn mssql_err(e: impl Into<bb8_tiberius::Error>) -> IdeError {
    match e.into() {
        bb8_tiberius::Error::Tiberius(e) => match &e {
            tiberius::error::Error::Server(d) => {
                let code = d.code();
                // Wrong login: SQL Server 18456.
                let auth = matches!(code, 18452 | 18456 | 18470);
                IdeError::new(
                    if auth { ErrorKind::PermissionDenied } else { ErrorKind::InvalidInput },
                    format!("[{code}] {}", d.message()),
                )
            }
            tiberius::error::Error::Io { .. } | tiberius::error::Error::Tls(_) => {
                IdeError::new(ErrorKind::Io, e.to_string())
            }
            _ => IdeError::new(ErrorKind::Protocol, e.to_string()),
        },
        bb8_tiberius::Error::Io(e) => IdeError::new(ErrorKind::Io, e.to_string()),
    }
}

fn mssql_pool_err(e: bb8::RunError<bb8_tiberius::Error>) -> IdeError {
    match e {
        bb8::RunError::User(e) => mssql_err(e),
        bb8::RunError::TimedOut => IdeError::new(ErrorKind::Io, "timed out connecting to the database"),
    }
}

// --- Oracle / Db2, through ODBC (`odbc-api`) -------------------------------------------
//
// Neither has a pure-Rust driver; both go through the system's ODBC driver manager and
// the vendor's own ODBC driver (installed separately, not bundled). ODBC calls are
// blocking, so every one of them runs in `spawn_blocking`. The connection is `take()`n
// out of its slot first and put back only once the blocking call returns: if the
// `execute`/`rows` future is dropped for cancellation, the slot is left `None` (so the
// pool discards it instead of reusing a connection whose statement may still be
// running on its abandoned blocking thread, which simply disconnects once that
// statement finally returns).

/// bb8 marks a connection broken by looking for `None` after an operation `take()`s it.
pub struct OdbcConnection(Option<odbc_api::Connection<'static>>);

impl std::ops::Deref for OdbcConnection {
    type Target = odbc_api::Connection<'static>;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("live ODBC connection")
    }
}

/// Why connecting, or checking liveness, failed.
#[derive(Debug)]
pub enum OdbcManageError {
    Odbc(odbc_api::Error),
    /// The blocking task panicked.
    Join(tokio::task::JoinError),
}

pub struct OdbcConnectionManager {
    pub connection_string: String,
    /// Statements run on every new connection (startup script…).
    pub init: Arc<Vec<String>>,
}

/// Runs blocking ODBC work on `wrapper`'s connection: taken out first, so cancelling the
/// caller (dropping this future) leaves `wrapper` holding `None` (see module notes above).
async fn odbc_run<T: Send + 'static>(
    wrapper: &mut OdbcConnection,
    f: impl FnOnce(&odbc_api::Connection<'static>) -> IdeResult<T> + Send + 'static,
) -> IdeResult<T> {
    let conn = wrapper.0.take().ok_or_else(|| IdeError::new(ErrorKind::Io, "the connection was discarded"))?;
    let (conn, r) = tokio::task::spawn_blocking(move || {
        let r = f(&conn);
        (conn, r)
    })
    .await
    .map_err(|e| IdeError::internal(e.to_string()))?;
    wrapper.0 = Some(conn);
    r
}

impl ManageConnection for OdbcConnectionManager {
    type Connection = OdbcConnection;
    type Error = OdbcManageError;

    async fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let connection_string = self.connection_string.clone();
        let init = self.init.clone();
        tokio::task::spawn_blocking(move || {
            let env = odbc_api::environment().map_err(OdbcManageError::Odbc)?;
            let conn = env
                .connect_with_connection_string(&connection_string, OdbcConnectionOptions::default())
                .map_err(OdbcManageError::Odbc)?;
            for sql in init.iter() {
                conn.execute(sql, (), None).map_err(OdbcManageError::Odbc)?;
            }
            Ok(OdbcConnection(Some(conn)))
        })
        .await
        .map_err(OdbcManageError::Join)?
    }

    async fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        let c = conn.0.take().ok_or(OdbcManageError::Odbc(odbc_api::Error::NoDiagnostics { function: "is_valid" }))?;
        let (c, dead) = tokio::task::spawn_blocking(move || {
            let dead = c.is_dead();
            (c, dead)
        })
        .await
        .map_err(OdbcManageError::Join)?;
        conn.0 = Some(c);
        match dead.map_err(OdbcManageError::Odbc)? {
            false => Ok(()),
            true => Err(OdbcManageError::Odbc(odbc_api::Error::NoDiagnostics { function: "is_valid" })),
        }
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        conn.0.is_none()
    }
}

fn odbc_err(e: odbc_api::Error) -> IdeError {
    if let odbc_api::Error::Diagnostics { record, .. } = &e {
        // SQLSTATE class 28 ("invalid authorization specification") covers bad credentials
        // for every ODBC driver, Oracle's and Db2's included.
        let auth = record.state.as_str().starts_with("28");
        return IdeError::new(
            if auth { ErrorKind::PermissionDenied } else { ErrorKind::InvalidInput },
            record.to_string(),
        );
    }
    IdeError::new(ErrorKind::Protocol, e.to_string())
}

fn odbc_manage_err(e: OdbcManageError) -> IdeError {
    match e {
        OdbcManageError::Odbc(e) => odbc_err(e),
        OdbcManageError::Join(e) => IdeError::internal(e.to_string()),
    }
}

fn odbc_pool_err(e: bb8::RunError<OdbcManageError>) -> IdeError {
    match e {
        bb8::RunError::User(e) => odbc_manage_err(e),
        bb8::RunError::TimedOut => IdeError::new(ErrorKind::Io, "timed out connecting to the database"),
    }
}

/// The ODBC connection string for `source` (Oracle's `DBQ=host:port/service` Easy Connect
/// form, or Db2's `DATABASE=/HOSTNAME=/PORT=` keywords), with `setup.params` appended as
/// extra `key=value` attributes (the data source's Advanced tab properties).
fn odbc_connection_string(
    source: &DataSource,
    password: Option<&str>,
    params: &[(String, String)],
) -> IdeResult<String> {
    let driver_name = source.odbc_driver.trim();
    if driver_name.is_empty() {
        return Err(IdeError::invalid(
            "set the ODBC driver name (Advanced tab): the name registered with the system's ODBC \
             driver manager, e.g. `odbcinst -q -d`. OpenIDEA does not bundle an Oracle or Db2 driver.",
        ));
    }
    let mut s = format!("Driver={{{}}};", odbc_escape(driver_name));
    let host = if source.host.trim().is_empty() { "localhost" } else { source.host.trim() };
    match source.driver {
        DbDriver::Oracle => {
            let service = source.database.trim();
            let port = source.port.unwrap_or(1521);
            let dbq = if service.is_empty() { format!("{host}:{port}") } else { format!("{host}:{port}/{service}") };
            s += &format!("DBQ={};", odbc_escape(&dbq));
        }
        DbDriver::Db2 => {
            s += &format!(
                "DATABASE={};HOSTNAME={};PORT={};PROTOCOL=TCPIP;",
                odbc_escape(source.database.trim()),
                odbc_escape(host),
                source.port.unwrap_or(50000)
            );
            if !matches!(source.ssl, DbSslMode::Disable) {
                s += "SECURITY=SSL;";
            }
        }
        _ => unreachable!("odbc_connection_string is only called for Oracle and Db2"),
    }
    if !source.user.trim().is_empty() {
        s += &format!("UID={};", odbc_escape(source.user.trim()));
    }
    if let Some(p) = password {
        s += &format!("PWD={};", odbc_escape(p));
    }
    for (k, v) in params {
        s += &format!("{}={};", odbc_escape(k), odbc_escape(v));
    }
    Ok(s)
}

/// A human readable type name for [`ResultColumn::type_name`] (the ODBC standard type,
/// not the vendor's native one — unlike [`sql::columns`], which reads the catalog).
fn odbc_type_name(ty: OdbcDataType) -> String {
    let len = |l: Option<std::num::NonZeroUsize>| l.map_or_else(|| "?".to_owned(), |n| n.to_string());
    match ty {
        OdbcDataType::Unknown => "UNKNOWN".to_owned(),
        OdbcDataType::Char { length } => format!("CHAR({})", len(length)),
        OdbcDataType::WChar { length } => format!("NCHAR({})", len(length)),
        OdbcDataType::Varchar { length } => format!("VARCHAR({})", len(length)),
        OdbcDataType::WVarchar { length } => format!("NVARCHAR({})", len(length)),
        OdbcDataType::LongVarchar { .. } => "LONG VARCHAR".to_owned(),
        OdbcDataType::WLongVarchar { .. } => "LONG NVARCHAR".to_owned(),
        OdbcDataType::Numeric { precision, scale } => format!("NUMERIC({precision},{scale})"),
        OdbcDataType::Decimal { precision, scale } => format!("DECIMAL({precision},{scale})"),
        OdbcDataType::Integer => "INTEGER".to_owned(),
        OdbcDataType::SmallInt => "SMALLINT".to_owned(),
        OdbcDataType::Float { precision } => format!("FLOAT({precision})"),
        OdbcDataType::Real => "REAL".to_owned(),
        OdbcDataType::Double => "DOUBLE".to_owned(),
        OdbcDataType::Date => "DATE".to_owned(),
        OdbcDataType::Time { .. } => "TIME".to_owned(),
        OdbcDataType::Timestamp { .. } => "TIMESTAMP".to_owned(),
        OdbcDataType::BigInt => "BIGINT".to_owned(),
        OdbcDataType::TinyInt => "TINYINT".to_owned(),
        OdbcDataType::Bit => "BIT".to_owned(),
        OdbcDataType::Varbinary { length } => format!("VARBINARY({})", len(length)),
        OdbcDataType::Binary { length } => format!("BINARY({})", len(length)),
        OdbcDataType::LongVarbinary { .. } => "LONG VARBINARY".to_owned(),
        OdbcDataType::Other { data_type, .. } => format!("{data_type:?}"),
    }
}

/// The text of one cell: the driver already hex-encodes true binary data when it is
/// bound (as here) as character data, per the ODBC conversion rules; `binary` only
/// controls the `0x` prefix this crate's grids otherwise use for such values.
fn odbc_cell(bytes: &[u8], binary: bool) -> String {
    let text = String::from_utf8_lossy(bytes);
    if binary { format!("0x{}", text.to_ascii_uppercase()) } else { text.into_owned() }
}

/// A generous but bounded per-cell size: `TextRowSet` needs an upper bound to size its
/// buffers, and some drivers report none for LOB columns.
const ODBC_MAX_CELL_BYTES: usize = 1 << 20;

/// Runs `sql` on `conn` and reads up to `max` rows as text (see [`Pool::rows`] / [`Held::execute`]).
fn odbc_fetch(conn: &odbc_api::Connection<'static>, sql: &str, max: usize) -> IdeResult<Fetched> {
    let mut stmt = conn.preallocate().map_err(odbc_err)?;
    let Some(mut cursor) = stmt.execute(sql, ()).map_err(odbc_err)? else {
        let affected = stmt.row_count().map_err(odbc_err)?;
        return Ok((Vec::new(), Vec::new(), affected.map(|n| n as u64), false, Some(Instant::now())));
    };
    let first = Some(Instant::now());
    let n = cursor.num_result_cols().map_err(odbc_err)? as usize;
    let mut columns = Vec::with_capacity(n);
    let mut binary = Vec::with_capacity(n);
    for i in 1..=n as u16 {
        let name = cursor.col_name(i).map_err(odbc_err)?;
        let ty = cursor.col_data_type(i).map_err(odbc_err)?;
        columns.push(ResultColumn { name, type_name: odbc_type_name(ty) });
        binary.push(matches!(
            ty,
            OdbcDataType::Binary { .. } | OdbcDataType::Varbinary { .. } | OdbcDataType::LongVarbinary { .. }
        ));
    }
    let buffer = TextRowSet::for_cursor(max + 1, &mut cursor, Some(ODBC_MAX_CELL_BYTES)).map_err(odbc_err)?;
    let mut block = cursor.bind_buffer(buffer).map_err(odbc_err)?;
    let mut rows = Vec::new();
    let mut has_more = false;
    if let Some(batch) = block.fetch().map_err(odbc_err)? {
        let total = batch.num_rows();
        has_more = total > max;
        for r in 0..total.min(max) {
            rows.push((0..n).map(|c| batch.at(c, r).map(|b| odbc_cell(b, binary[c]))).collect());
        }
    }
    Ok((columns, rows, None, has_more, first))
}

const DAYS_1900_01_01: i64 = 693_595;

/// Formats a proleptic Gregorian date given days since `0001-01-01`.
fn format_date(days: i64) -> String {
    let z = days + 306;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}")
}

fn format_hms(secs: u64, frac: u64, scale: u8) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if scale == 0 || frac == 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        let frac = format!("{frac:0width$}", width = usize::from(scale));
        format!("{h:02}:{m:02}:{s:02}.{}", frac.trim_end_matches('0'))
    }
}

fn format_time(t: tiberius::time::Time) -> String {
    let scale = t.scale().min(7);
    let div = 10u64.pow(u32::from(scale));
    format_hms(t.increments() / div, t.increments() % div, scale)
}

fn format_numeric(n: tiberius::numeric::Numeric) -> String {
    let scale = u32::from(n.scale());
    if scale == 0 {
        return n.value().to_string();
    }
    let sign = if n.value() < 0 { "-" } else { "" };
    let abs = n.value().unsigned_abs();
    let div = 10u128.pow(scale);
    format!("{sign}{}.{:0width$}", abs / div, abs % div, width = scale as usize)
}

fn tiberius_cell(cell: &tiberius::ColumnData<'_>, ty: tiberius::ColumnType) -> Option<String> {
    use tiberius::ColumnData as C;
    match cell {
        C::U8(v) => v.map(|v| v.to_string()),
        C::I16(v) => v.map(|v| v.to_string()),
        C::I32(v) => v.map(|v| v.to_string()),
        C::I64(v) => v.map(|v| v.to_string()),
        C::F32(v) => v.map(|v| v.to_string()),
        C::F64(v) if matches!(ty, tiberius::ColumnType::Money | tiberius::ColumnType::Money4) => {
            // Beyond ~9e11, money precision is limited by the driver's f64 decoding.
            v.map(|v| format!("{:.4}", v))
        }
        C::F64(v) => v.map(|v| v.to_string()),
        C::Bit(v) => v.map(|v| v.to_string()),
        C::String(v) => v.as_deref().map(str::to_owned),
        C::Guid(v) => v.map(|v| v.to_string()),
        C::Binary(v) => v.as_deref().map(hex),
        C::Numeric(v) => v.map(format_numeric),
        C::Xml(v) => v.as_deref().map(|x| x.as_ref().to_owned()),
        C::Date(v) => v.map(|d| format_date(i64::from(d.days()))),
        C::Time(v) => v.map(format_time),
        C::DateTime(v) => v.map(|dt| {
            let ms = (u64::from(dt.seconds_fragments()) * 1000 + 150) / 300;
            format!("{} {}", format_date(DAYS_1900_01_01 + i64::from(dt.days())), format_hms(ms / 1000, ms % 1000, 3))
        }),
        C::SmallDateTime(v) => v.map(|dt| {
            let secs = u64::from(dt.seconds_fragments()) * 60;
            format!("{} {}", format_date(DAYS_1900_01_01 + i64::from(dt.days())), format_hms(secs, 0, 0))
        }),
        C::DateTime2(v) => {
            v.map(|dt| format!("{} {}", format_date(i64::from(dt.date().days())), format_time(dt.time())))
        }
        C::DateTimeOffset(v) => v.map(|dto| {
            let dt = dto.datetime2();
            let t = dt.time();
            let scale = t.scale().min(7);
            let div = 10u64.pow(u32::from(scale));
            let utc_secs = i64::from(dt.date().days()) * 86_400 + (t.increments() / div) as i64;
            let local_secs = utc_secs + i64::from(dto.offset()) * 60;
            let days = local_secs.div_euclid(86_400);
            let secs = local_secs.rem_euclid(86_400) as u64;
            let sign = if dto.offset() < 0 { '-' } else { '+' };
            let off = dto.offset().unsigned_abs();
            format!(
                "{} {}{sign}{:02}:{:02}",
                format_date(days),
                format_hms(secs, t.increments() % div, scale),
                off / 60,
                off % 60
            )
        }),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// How [`cell`] shows a value whose bytes are not text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cells {
    /// Result grids: binary values as `0x…` hex.
    Display,
    /// Catalog queries: names and definitions, so bytes are read as UTF-8 whatever the
    /// column's type (MySQL flags `utf8mb3_bin` catalog columns BINARY, and sends every
    /// column as binary under `character_set_results = binary`).
    Text,
}

/// Whether a value of type `ty` is binary data (see [`binary_type`]).
fn is_binary<DB: sqlx::Database>(ty: &DB::TypeInfo) -> bool
where
    String: sqlx::Type<DB>,
{
    binary_type(ty.name(), <String as sqlx::Type<DB>>::compatible(ty))
}

/// A BLOB / BINARY type that the driver does not read as text: MySQL flags text in a
/// binary collation (`utf8mb4_bin`) BINARY too, and only the `binary` collation is bytes.
fn binary_type(name: &str, text: bool) -> bool {
    let name = name.to_ascii_uppercase();
    (name.contains("BLOB") || name.contains("BINARY")) && !text
}

/// Bytes as text: UTF-8, else hex (grids) or with invalid sequences replaced (catalog names).
fn bytes_text(bytes: Vec<u8>, mode: Cells) -> String {
    match (String::from_utf8(bytes), mode) {
        (Ok(s), _) => s,
        (Err(e), Cells::Display) => hex(e.as_bytes()),
        (Err(e), Cells::Text) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// The text of column `i` of `row`.
fn cell<R: Row>(row: &R, i: usize, mode: Cells) -> Option<String>
where
    usize: ColumnIndex<R>,
    String: sqlx::Type<R::Database>,
    for<'r> String: Decode<'r, R::Database>,
    for<'r> Vec<u8>: Decode<'r, R::Database>,
{
    let raw = row.try_get_raw(i).ok()?;
    if raw.is_null() {
        return None;
    }
    if mode == Cells::Display && is_binary::<R::Database>(&raw.type_info()) {
        return <Vec<u8> as Decode<R::Database>>::decode(raw).ok().map(|b| hex(&b));
    }
    match <String as Decode<R::Database>>::decode(raw) {
        Ok(s) => Some(s),
        Err(_) => {
            let raw = row.try_get_raw(i).ok()?;
            <Vec<u8> as Decode<R::Database>>::decode(raw).ok().map(|b| bytes_text(b, mode))
        }
    }
}

fn result_columns<R: Row>(row: &R) -> Vec<ResultColumn> {
    row.columns()
        .iter()
        .map(|c| ResultColumn { name: c.name().to_owned(), type_name: c.type_info().name().to_owned() })
        .collect()
}

/// Streams the results of `$sql` on executor `$exec`, keeping at most `$max` rows
/// with their cells read as `$mode`.
macro_rules! fetch {
    ($exec:expr, $sql:expr, $max:expr, $mode:expr) => {{
        use futures::StreamExt;
        let mut stream = sqlx::raw_sql(sqlx::AssertSqlSafe($sql.to_owned())).fetch_many($exec);
        let mut columns: Vec<ResultColumn> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        let mut affected: Option<u64> = None;
        let mut has_more = false;
        let mut failure = None;
        let mut first: Option<Instant> = None;
        while let Some(item) = stream.next().await {
            first.get_or_insert_with(Instant::now);
            match item {
                Ok(sqlx::Either::Left(done)) => affected = Some(affected.unwrap_or(0) + done.rows_affected()),
                Ok(sqlx::Either::Right(row)) => {
                    if columns.is_empty() {
                        columns = result_columns(&row);
                    }
                    if rows.len() >= $max {
                        has_more = true;
                        break;
                    }
                    rows.push((0..row.len()).map(|i| cell(&row, i, $mode)).collect());
                }
                Err(e) => {
                    failure = Some(db_err(e));
                    break;
                }
            }
        }
        drop(stream);
        match failure {
            Some(e) => Err(e),
            None => Ok((columns, rows, affected, has_more, first)),
        }
    }};
}

/// Columns, rows, rows affected, more rows beyond the limit, time of the first answer.
type Fetched = (Vec<ResultColumn>, Vec<Vec<Option<String>>>, Option<u64>, bool, Option<Instant>);

async fn mssql_command(conn: &mut bb8_tiberius::rt::Client, sql: &str) -> IdeResult<()> {
    // Transaction control must be a batch: sp_executesql rejects a changed transaction count.
    conn.simple_query(sql).await.map_err(mssql_err)?.into_results().await.map_err(mssql_err)?;
    Ok(())
}

/// Runs [`crate::conn::begin_statement`]'s `"BEGIN"` sentinel, `"COMMIT"` or `"ROLLBACK"`
/// (the only texts [`Held::command`] ever passes here) as the matching ODBC connection
/// attribute / transaction call, since Oracle and Db2 have no such SQL statements.
///
/// The caller (`Store::run` / `Store::finish_transaction`) always returns the connection
/// to the pool right after a `"COMMIT"` / `"ROLLBACK"` (a new one is acquired and put
/// back into manual-commit mode for the data source's next transaction), so both restore
/// autocommit before this function returns — a connection must never sit in the pool with
/// autocommit left off, or an unrelated one-off statement would silently run inside it.
async fn odbc_command(conn: &mut OdbcConnection, sql: &str) -> IdeResult<()> {
    let sql = sql.to_owned();
    odbc_run(conn, move |c| {
        match sql.as_str() {
            "BEGIN" => c.set_autocommit(false),
            "COMMIT" => c.commit().and_then(|()| c.set_autocommit(true)),
            "ROLLBACK" => c.rollback().and_then(|()| c.set_autocommit(true)),
            // Not reached: see this function's doc comment.
            _ => return Err(IdeError::internal(format!("unexpected command: {sql}"))),
        }
        .map_err(odbc_err)
    })
    .await
}

async fn mssql_fetch(conn: &mut bb8_tiberius::rt::Client, sql: &str, max: usize) -> IdeResult<Fetched> {
    let mut stream = conn.simple_query(sql).await.map_err(mssql_err)?;
    let first = Some(Instant::now());
    let columns = stream
        .columns()
        .await
        .map_err(mssql_err)?
        .unwrap_or_default()
        .iter()
        .map(|col| ResultColumn { name: col.name().to_owned(), type_name: format!("{:?}", col.column_type()) })
        .collect();
    let mut rows = Vec::new();
    let mut has_more = false;
    while let Some(item) = stream.next().await {
        if let tiberius::QueryItem::Row(row) = item.map_err(mssql_err)? {
            if rows.len() >= max {
                has_more = true;
                break;
            }
            rows.push(row.cells().map(|(col, cell)| tiberius_cell(cell, col.column_type())).collect());
        }
    }
    Ok((columns, rows, None, has_more, first))
}

/// The server-side id of a connection (to cancel its statement from another one).
macro_rules! backend_id {
    ($conn:expr, $sql:expr) => {{
        let fetched: IdeResult<Fetched> = fetch!($conn, $sql, 1, Cells::Text);
        fetched
            .ok()
            .and_then(|f| f.1.into_iter().next())
            .and_then(|r| r.into_iter().next().flatten())
            .filter(|id| id.chars().all(|c| c.is_ascii_digit()))
    }};
}

fn looks_like_query(sql: &str) -> bool {
    let first = sql
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("--"))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(
        first.as_str(),
        "SELECT" | "WITH" | "VALUES" | "TABLE" | "SHOW" | "PRAGMA" | "EXPLAIN" | "DESCRIBE" | "DESC"
    )
}

fn file_setting(path: &str) -> Option<PathBuf> {
    let path = path.trim();
    (!path.is_empty()).then(|| ide_ssh::expand(path))
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// A MySQL `SET` value: numbers and keywords as they are, anything else quoted.
fn mysql_value(v: &str) -> String {
    let keyword = ["ON", "OFF", "TRUE", "FALSE", "DEFAULT"].iter().any(|k| k.eq_ignore_ascii_case(v));
    if keyword || v.parse::<f64>().is_ok() {
        v.to_owned()
    } else {
        format!("'{}'", v.replace('\\', "\\\\").replace('\'', "''"))
    }
}

/// How the Options and Advanced tabs reach a connection.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Setup {
    /// PostgreSQL run-time parameters (startup `options`), SQLite pragmas.
    pub params: Vec<(String, String)>,
    /// Run on every new connection: MySQL session settings, then the startup script.
    pub statements: Vec<String>,
    pub application_name: Option<String>,
    pub charset: Option<String>,
    pub collation: Option<String>,
    pub statement_cache: Option<usize>,
    pub connect_timeout: Option<Duration>,
}

impl Setup {
    pub fn of(source: &DataSource) -> IdeResult<Self> {
        let mut s = Setup::default();
        let o = &source.options;
        match source.driver {
            DbDriver::Postgres => {
                if source.read_only {
                    s.params.push(("default_transaction_read_only".into(), "on".into()));
                }
                if !o.default_schema.trim().is_empty() {
                    s.params.push(("search_path".into(), o.default_schema.trim().into()));
                }
                if !o.time_zone.trim().is_empty() {
                    s.params.push(("TimeZone".into(), o.time_zone.trim().into()));
                }
            }
            DbDriver::Mysql if source.read_only => s.statements.push("SET SESSION TRANSACTION READ ONLY".into()),
            _ => {}
        }
        for p in &source.properties {
            let value = p.value.trim();
            let name = p.name.trim().replace('-', "_");
            if name.is_empty() {
                continue;
            }
            if !is_identifier(&name) {
                return Err(IdeError::invalid(format!("invalid driver property name: {}", p.name.trim())));
            }
            let number = |what: &str| {
                value.parse::<u64>().map_err(|_| IdeError::invalid(format!("{what} must be a whole number: {value}")))
            };
            match (source.driver, name.to_ascii_lowercase().as_str()) {
                (_, "statement_cache_capacity") => s.statement_cache = Some(number(&name)? as usize),
                (_, "connect_timeout") => s.connect_timeout = Some(Duration::from_secs(number(&name)?)),
                (DbDriver::Postgres, "application_name") => s.application_name = Some(value.to_owned()),
                (DbDriver::Mysql, "charset") => s.charset = Some(value.to_owned()),
                (DbDriver::Mysql, "collation") => s.collation = Some(value.to_owned()),
                (DbDriver::Mysql, _) => s.statements.push(format!("SET SESSION {name} = {}", mysql_value(value))),
                _ => s.params.push((name, value.to_owned())),
            }
        }
        let script = o.startup_script.trim();
        if !script.is_empty() {
            s.statements.extend(crate::split::split(script, source.driver).into_iter().map(|st| st.text));
        }
        Ok(s)
    }
}

impl Pool {
    pub fn driver(&self) -> DbDriver {
        match self {
            Pool::Sqlite(_) => DbDriver::Sqlite,
            Pool::Postgres(_) => DbDriver::Postgres,
            Pool::Mysql(_) => DbDriver::Mysql,
            Pool::SqlServer(_) => DbDriver::SqlServer,
            Pool::Oracle(_) => DbDriver::Oracle,
            Pool::Db2(_) => DbDriver::Db2,
        }
    }

    /// Opens a pool and one connection (so bad settings fail here).
    pub async fn connect(source: &DataSource, password: Option<&str>, root: Option<&Path>) -> IdeResult<Self> {
        const MAX: u32 = 4;
        let setup = Setup::of(source)?;
        let timeout = setup.connect_timeout.unwrap_or(Duration::from_secs(30));
        let init = Arc::new(setup.statements);
        // Runs the session statements (startup script…) on every new connection.
        macro_rules! after_connect {
            () => {{
                let init = init.clone();
                move |conn, _| {
                    let init = init.clone();
                    Box::pin(async move {
                        for sql in init.iter() {
                            sqlx::raw_sql(sqlx::AssertSqlSafe(sql.clone())).execute(&mut *conn).await?;
                        }
                        Ok(())
                    })
                }
            }};
        }
        match source.driver {
            DbDriver::Sqlite => {
                let file = source.file.as_deref().ok_or_else(|| IdeError::invalid("no database file"))?;
                let file = match root {
                    Some(r) if file.is_relative() => r.join(file),
                    _ => file.to_path_buf(),
                };
                let mut opts =
                    SqliteConnectOptions::new().filename(&file).create_if_missing(false).read_only(source.read_only);
                for (k, v) in setup.params {
                    opts = opts.pragma(k, v);
                }
                if let Some(n) = setup.statement_cache {
                    opts = opts.statement_cache_capacity(n);
                }
                let pool = SqlitePoolOptions::new()
                    .max_connections(MAX)
                    .acquire_timeout(timeout)
                    .after_connect(after_connect!())
                    .connect_with(opts)
                    .await
                    .map_err(db_err)?;
                Ok(Pool::Sqlite(pool))
            }
            DbDriver::Postgres => {
                let mut opts = PgConnectOptions::new()
                    .host(&source.host)
                    .port(source.port.unwrap_or(5432))
                    .username(&source.user)
                    .application_name(setup.application_name.as_deref().unwrap_or("OpenIDEA"))
                    .ssl_mode(match source.ssl {
                        DbSslMode::Disable => PgSslMode::Disable,
                        DbSslMode::Prefer => PgSslMode::Prefer,
                        DbSslMode::Require => PgSslMode::Require,
                        DbSslMode::VerifyCa => PgSslMode::VerifyCa,
                        DbSslMode::VerifyFull => PgSslMode::VerifyFull,
                    });
                if let Some(f) = file_setting(&source.ssl_ca_file) {
                    opts = opts.ssl_root_cert(f);
                }
                if let Some(f) = file_setting(&source.ssl_cert_file) {
                    opts = opts.ssl_client_cert(f);
                }
                if let Some(f) = file_setting(&source.ssl_key_file) {
                    opts = opts.ssl_client_key(f);
                }
                if !source.database.is_empty() {
                    opts = opts.database(&source.database);
                }
                if let Some(p) = password {
                    opts = opts.password(p);
                }
                if !setup.params.is_empty() {
                    opts = opts.options(setup.params);
                }
                if let Some(n) = setup.statement_cache {
                    opts = opts.statement_cache_capacity(n);
                }
                let pool = PgPoolOptions::new()
                    .max_connections(MAX)
                    .acquire_timeout(timeout)
                    .after_connect(after_connect!())
                    .connect_with(opts)
                    .await
                    .map_err(db_err)?;
                Ok(Pool::Postgres(pool))
            }
            DbDriver::Mysql => {
                let mut opts = MySqlConnectOptions::new()
                    .host(&source.host)
                    .port(source.port.unwrap_or(3306))
                    .username(&source.user)
                    .ssl_mode(match source.ssl {
                        DbSslMode::Disable => MySqlSslMode::Disabled,
                        DbSslMode::Prefer => MySqlSslMode::Preferred,
                        DbSslMode::Require => MySqlSslMode::Required,
                        DbSslMode::VerifyCa => MySqlSslMode::VerifyCa,
                        DbSslMode::VerifyFull => MySqlSslMode::VerifyIdentity,
                    });
                if let Some(f) = file_setting(&source.ssl_ca_file) {
                    opts = opts.ssl_ca(f);
                }
                if let Some(f) = file_setting(&source.ssl_cert_file) {
                    opts = opts.ssl_client_cert(f);
                }
                if let Some(f) = file_setting(&source.ssl_key_file) {
                    opts = opts.ssl_client_key(f);
                }
                if !source.database.is_empty() {
                    opts = opts.database(&source.database);
                }
                if let Some(p) = password {
                    opts = opts.password(p);
                }
                if let Some(c) = &setup.charset {
                    opts = opts.charset(c);
                }
                if let Some(c) = &setup.collation {
                    opts = opts.collation(c);
                }
                if let Some(n) = setup.statement_cache {
                    opts = opts.statement_cache_capacity(n);
                }
                let tz = source.options.time_zone.trim();
                if !tz.is_empty() {
                    opts = opts.timezone(Some(tz.to_owned()));
                }
                let pool = MySqlPoolOptions::new()
                    .max_connections(MAX)
                    .acquire_timeout(timeout)
                    .after_connect(after_connect!())
                    .connect_with(opts)
                    .await
                    .map_err(db_err)?;
                Ok(Pool::Mysql(pool))
            }
            DbDriver::SqlServer => {
                use tiberius::Config;
                let mut config = Config::new();
                config.host(&source.host);
                config.port(source.port.unwrap_or(1433));
                if !source.user.is_empty() {
                    config.authentication(tiberius::AuthMethod::sql_server(
                        source.user.clone(),
                        password.unwrap_or_default().to_owned(),
                    ));
                }
                if !source.database.is_empty() {
                    config.database(&source.database);
                }
                if matches!(source.ssl, DbSslMode::Disable | DbSslMode::Prefer) {
                    config.trust_cert();
                }
                let manager = MssqlConnectionManager(bb8_tiberius::ConnectionManager::new(config));
                let pool = bb8::Pool::builder()
                    .max_size(MAX)
                    .connection_timeout(timeout)
                    .build(manager)
                    .await
                    .map_err(mssql_err)?;
                let mut conn = pool.get_owned().await.map_err(mssql_pool_err)?;
                conn.query("SELECT 1", &[]).await.map_err(mssql_err)?.into_results().await.map_err(mssql_err)?;
                Ok(Pool::SqlServer(pool))
            }
            DbDriver::Oracle | DbDriver::Db2 => {
                let connection_string = odbc_connection_string(source, password, &setup.params)?;
                let manager = OdbcConnectionManager { connection_string, init };
                let pool = bb8::Pool::builder()
                    .max_size(MAX)
                    .connection_timeout(timeout)
                    .build(manager)
                    .await
                    .map_err(odbc_manage_err)?;
                // Open one connection now so bad settings (and a missing ODBC driver) fail here.
                drop(pool.get_owned().await.map_err(odbc_pool_err)?);
                Ok(if source.driver == DbDriver::Oracle { Pool::Oracle(pool) } else { Pool::Db2(pool) })
            }
        }
    }

    pub async fn close(&self) {
        match self {
            Pool::SqlServer(_) | Pool::Oracle(_) | Pool::Db2(_) => {}
            _ => with_pool!(self, p => p.close().await),
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            Pool::SqlServer(_) | Pool::Oracle(_) | Pool::Db2(_) => false,
            _ => with_pool!(self, p => p.is_closed()),
        }
    }

    /// Runs `sql` (no row limit beyond `max`), for introspection: cells are text (see [`Cells::Text`]).
    pub async fn rows(&self, sql: &str, max: usize) -> IdeResult<Vec<Vec<Option<String>>>> {
        let fetched: IdeResult<Fetched> = match self {
            Pool::SqlServer(p) => {
                let mut conn = p.get_owned().await.map_err(mssql_pool_err)?;
                mssql_fetch(&mut conn, sql, max).await
            }
            Pool::Oracle(p) | Pool::Db2(p) => {
                let mut conn = p.get_owned().await.map_err(odbc_pool_err)?;
                let sql = sql.to_owned();
                odbc_run(&mut conn, move |c| odbc_fetch(c, &sql, max)).await
            }
            _ => with_pool!(self, p => fetch!(p, sql, max, Cells::Text)),
        };
        Ok(fetched?.1)
    }

    /// Runs one statement and keeps at most `max_rows` rows. Cancelling stops
    /// the statement on the server (PostgreSQL, MySQL) and drops the connection.
    pub async fn execute(&self, sql: &str, max_rows: usize, cancel: &CancellationToken) -> IdeResult<QueryResult> {
        let mut conn = Held::acquire(self).await?;
        let r = conn.execute(self, sql, max_rows, cancel).await;
        if cancel.is_cancelled() {
            conn.discard().await;
            return Err(IdeError::cancelled());
        }
        r
    }

    /// Runs `statements` in one transaction; each must change `expect_one`
    /// row when flagged (table editor submits).
    pub async fn transaction(&self, statements: &[(String, bool)]) -> IdeResult<u64> {
        match self {
            Pool::SqlServer(p) => {
                let mut conn = p.get_owned().await.map_err(mssql_pool_err)?;
                let result = async {
                    mssql_command(&mut conn, "BEGIN TRAN").await?;
                    let mut total = 0;
                    for (sql, expect_one) in statements {
                        let n = conn.execute(sql, &[]).await.map_err(mssql_err)?.total();
                        if *expect_one && n != 1 {
                            return Err(IdeError::invalid(format!(
                                "{sql}: expected to change 1 row, changed {n}; nothing was saved"
                            )));
                        }
                        total += n;
                    }
                    mssql_command(&mut conn, "COMMIT TRAN").await?;
                    Ok(total)
                }
                .await;
                if result.is_err() && mssql_command(&mut conn, "IF @@TRANCOUNT > 0 ROLLBACK TRAN").await.is_err() {
                    // A failed rollback must never return an open transaction to the pool.
                    conn.0.take();
                }
                result
            }
            Pool::Oracle(p) | Pool::Db2(p) => {
                let mut conn = p.get_owned().await.map_err(odbc_pool_err)?;
                odbc_transaction(&mut conn, statements).await
            }
            _ => with_pool!(self, p => {
                let mut tx = p.begin().await.map_err(db_err)?;
                let mut total = 0;
                for (sql, expect_one) in statements {
                    let done = sqlx::raw_sql(sqlx::AssertSqlSafe(sql.clone())).execute(&mut *tx).await.map_err(db_err)?;
                    let n = done.rows_affected();
                    // MySQL reports 0 for an UPDATE that leaves the row unchanged.
                    if *expect_one && (n > 1 || (n == 0 && !matches!(self, Pool::Mysql(_)))) {
                        return Err(IdeError::invalid(format!("{sql}: expected to change 1 row, changed {n}; nothing was saved")));
                    }
                    total += n;
                }
                tx.commit().await.map_err(db_err)?;
                Ok(total)
            }),
        }
    }
}

/// Runs `statements` as one manual-commit transaction on `conn` (see [`Pool::transaction`]).
/// Unlike [`odbc_run`], a failed commit/rollback/autocommit restore discards the connection
/// (leaves it `None`) rather than risk pooling one stuck mid-transaction.
async fn odbc_transaction(
    conn: &mut bb8::PooledConnection<'static, OdbcConnectionManager>,
    statements: &[(String, bool)],
) -> IdeResult<u64> {
    let statements = statements.to_vec();
    let c = conn.0.take().ok_or_else(|| IdeError::new(ErrorKind::Io, "the connection was discarded"))?;
    let (c, r, keep) = tokio::task::spawn_blocking(move || {
        let r: IdeResult<u64> = (|| {
            c.set_autocommit(false).map_err(odbc_err)?;
            let mut total = 0u64;
            for (sql, expect_one) in &statements {
                let (_, _, affected, _, _) = odbc_fetch(&c, sql, 0)?;
                let n = affected.unwrap_or(0);
                if *expect_one && n != 1 {
                    return Err(IdeError::invalid(format!(
                        "{sql}: expected to change 1 row, changed {n}; nothing was saved"
                    )));
                }
                total += n;
            }
            Ok(total)
        })();
        let cleanup = if r.is_ok() { c.commit() } else { c.rollback() };
        let keep = cleanup.is_ok() && c.set_autocommit(true).is_ok();
        (c, r, keep)
    })
    .await
    .map_err(|e| IdeError::internal(e.to_string()))?;
    if keep {
        conn.0 = Some(c);
    }
    r
}

/// A connection taken out of a pool: for one statement, or held by a
/// manual-commit console across statements with its transaction open.
pub enum Held {
    Sqlite(PoolConnection<Sqlite>),
    Postgres(PoolConnection<Postgres>),
    Mysql(PoolConnection<MySql>),
    SqlServer(Box<bb8::PooledConnection<'static, MssqlConnectionManager>>),
    Oracle(Box<bb8::PooledConnection<'static, OdbcConnectionManager>>),
    Db2(Box<bb8::PooledConnection<'static, OdbcConnectionManager>>),
}

impl Held {
    pub async fn acquire(pool: &Pool) -> IdeResult<Self> {
        Ok(match pool {
            Pool::Sqlite(p) => Held::Sqlite(p.acquire().await.map_err(db_err)?),
            Pool::Postgres(p) => Held::Postgres(p.acquire().await.map_err(db_err)?),
            Pool::Mysql(p) => Held::Mysql(p.acquire().await.map_err(db_err)?),
            Pool::SqlServer(p) => Held::SqlServer(Box::new(p.get_owned().await.map_err(mssql_pool_err)?)),
            Pool::Oracle(p) => Held::Oracle(Box::new(p.get_owned().await.map_err(odbc_pool_err)?)),
            Pool::Db2(p) => Held::Db2(Box::new(p.get_owned().await.map_err(odbc_pool_err)?)),
        })
    }

    /// Runs a statement without results (`BEGIN`, `COMMIT`, `ROLLBACK`).
    pub async fn command(&mut self, sql: &str) -> IdeResult<()> {
        match self {
            Held::SqlServer(c) => mssql_command(c, sql).await,
            // Oracle and Db2 have no such SQL statements; see `conn::begin_statement`.
            Held::Oracle(c) | Held::Db2(c) => odbc_command(c, sql).await,
            other => {
                let sql = sqlx::AssertSqlSafe(sql.to_owned());
                match other {
                    Held::Sqlite(c) => sqlx::raw_sql(sql).execute(&mut **c).await.map(drop),
                    Held::Postgres(c) => sqlx::raw_sql(sql).execute(&mut **c).await.map(drop),
                    Held::Mysql(c) => sqlx::raw_sql(sql).execute(&mut **c).await.map(drop),
                    Held::SqlServer(_) | Held::Oracle(_) | Held::Db2(_) => unreachable!(),
                }
                .map_err(db_err)
            }
        }
    }

    /// Closes the connection instead of returning it to the pool (its statement was
    /// interrupted; an open transaction is rolled back by the server).
    pub async fn discard(self) {
        match self {
            Held::Sqlite(c) => drop(c.detach()),
            Held::Postgres(c) => {
                let _ = c.detach().close().await;
            }
            Held::Mysql(c) => {
                let _ = c.detach().close().await;
            }
            Held::SqlServer(mut c) => {
                c.0.take();
            }
            Held::Oracle(mut c) | Held::Db2(mut c) => {
                c.0.take();
            }
        }
    }

    /// Runs one statement on this connection (see [`Pool::execute`]); after a
    /// cancellation the caller must [`discard`](Self::discard) it.
    pub async fn execute(
        &mut self,
        pool: &Pool,
        sql: &str,
        max_rows: usize,
        cancel: &CancellationToken,
    ) -> IdeResult<QueryResult> {
        let started = Instant::now();
        // `$id`: the query naming the server process, `$kill`: how another connection stops it.
        macro_rules! run {
            ($conn:expr, $id:expr, $kill:expr) => {{
                let backend = match $id {
                    Some(q) => backend_id!(&mut **$conn, q),
                    None => None,
                };
                let r = tokio::select! {
                    r = async { fetch!(&mut **$conn, sql, max_rows, Cells::Display) } => r,
                    () = cancel.cancelled() => Err(IdeError::cancelled()),
                };
                if cancel.is_cancelled() {
                    if let Some(b) = backend {
                        let _ = pool.rows(&$kill(b), 1).await;
                    }
                    return Err(IdeError::cancelled());
                }
                let mut r = r?;
                if r.0.is_empty() && looks_like_query(sql) {
                    r.0 = (&mut **$conn)
                        .prepare(sqlx::AssertSqlSafe(sql.to_owned()).into_sql_str())
                        .await
                        .map(|s| statement_columns(&s))
                        .unwrap_or_default();
                }
                r
            }};
        }
        let fetched: Fetched = match self {
            Held::Sqlite(c) => run!(c, None::<&str>, |_: String| String::new()),
            Held::Postgres(c) => {
                run!(c, Some("SELECT pg_backend_pid()"), |b: String| format!("SELECT pg_cancel_backend({b})"))
            }
            Held::Mysql(c) => run!(c, Some("SELECT CONNECTION_ID()"), |b: String| format!("KILL QUERY {b}")),
            Held::SqlServer(c) => {
                // Tiberius has no attention/cancel API; stop reading and discard the connection.
                tokio::select! {
                    r = mssql_fetch(c, sql, max_rows) => r?,
                    () = cancel.cancelled() => return Err(IdeError::cancelled()),
                }
            }
            Held::Oracle(c) | Held::Db2(c) => {
                // odbc-api exposes no cancel API either; same fallback as SQL Server above.
                let text = sql.to_owned();
                tokio::select! {
                    r = odbc_run(c, move |conn| odbc_fetch(conn, &text, max_rows)) => r?,
                    () = cancel.cancelled() => return Err(IdeError::cancelled()),
                }
            }
        };
        let (columns, rows, affected, has_more, first) = fetched;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(QueryResult {
            statement: sql.to_owned(),
            affected: if columns.is_empty() { Some(affected.unwrap_or(0) as f64) } else { None },
            columns,
            rows,
            has_more,
            elapsed_ms,
            execution_ms: first.map_or(elapsed_ms, |t| t.duration_since(started).as_secs_f64() * 1000.0),
        })
    }
}

/// The statement opening a transaction. Oracle and Db2 have no such statement (a
/// transaction starts implicitly with the first DML); `"BEGIN"` is a sentinel
/// [`Held::command`] recognizes to call `SQLSetConnectAttr(SQL_ATTR_AUTOCOMMIT, off)`
/// instead of sending it as SQL.
pub fn begin_statement(driver: DbDriver) -> &'static str {
    match driver {
        DbDriver::Mysql => "START TRANSACTION",
        DbDriver::SqlServer => "BEGIN TRAN",
        DbDriver::Sqlite | DbDriver::Postgres | DbDriver::Oracle | DbDriver::Db2 => "BEGIN",
    }
}

fn statement_columns<S: sqlx::Statement>(s: &S) -> Vec<ResultColumn> {
    s.columns()
        .iter()
        .map(|c| ResultColumn { name: c.name().to_owned(), type_name: c.type_info().name().to_owned() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{DbOptions, DbProperty};

    #[test]
    fn sqlserver_cells_preserve_values_and_nulls() {
        use std::borrow::Cow;
        use tiberius::time::{Date, DateTime, DateTime2, DateTimeOffset, SmallDateTime, Time};
        use tiberius::{ColumnData as C, ColumnType as T, Uuid, numeric::Numeric};
        let dt = DateTime2::new(Date::new(693_595), Time::new(1234567, 7));
        let cases = [
            (C::U8(Some(255)), "255"),
            (C::I16(Some(-123)), "-123"),
            (C::I32(Some(-456)), "-456"),
            (C::I64(Some(i64::MAX)), "9223372036854775807"),
            (C::F32(Some(1.25)), "1.25"),
            (C::F64(Some(1.25)), "1.25"),
            (C::Bit(Some(true)), "true"),
            (C::String(Some(Cow::Borrowed("hello"))), "hello"),
            (C::Binary(Some(Cow::Borrowed(&[0, 0xAB, 0xFF]))), "0x00ABFF"),
            (
                C::Guid(Some(Uuid::parse_str("ABCDEF01-2345-6789-ABCD-EF0123456789").unwrap())),
                "abcdef01-2345-6789-abcd-ef0123456789",
            ),
            (C::Numeric(Some(Numeric::new_with_scale(-123, 5))), "-0.00123"),
            (C::Numeric(Some(Numeric::new_with_scale(12300, 4))), "1.2300"),
            (
                C::Numeric(Some(Numeric::new_with_scale(10i128.pow(38) - 1, 0))),
                "99999999999999999999999999999999999999",
            ),
            (C::Xml(Some(Cow::Owned(tiberius::xml::XmlData::new("<a/>")))), "<a/>"),
            (C::Date(Some(Date::new(0))), "0001-01-01"),
            (C::Date(Some(Date::new(3_652_058))), "9999-12-31"),
            (C::Time(Some(Time::new(452961234567, 7))), "12:34:56.1234567"),
            (C::DateTime(Some(DateTime::new(-1, 1))), "1899-12-31 00:00:00.003"),
            (C::DateTime(Some(DateTime::new(0, 2))), "1900-01-01 00:00:00.007"),
            (C::SmallDateTime(Some(SmallDateTime::new(0, 754))), "1900-01-01 12:34:00"),
            (C::DateTime2(Some(dt)), "1900-01-01 00:00:00.1234567"),
            (C::DateTimeOffset(Some(DateTimeOffset::new(dt, -60))), "1899-12-31 23:00:00.1234567-01:00"),
            (C::DateTimeOffset(Some(DateTimeOffset::new(dt, 330))), "1900-01-01 05:30:00.1234567+05:30"),
        ];
        for (cell, expected) in cases {
            assert_eq!(tiberius_cell(&cell, T::Null).as_deref(), Some(expected), "{cell:?}");
        }
        for cell in [
            C::U8(None),
            C::I16(None),
            C::I32(None),
            C::I64(None),
            C::F32(None),
            C::F64(None),
            C::Bit(None),
            C::String(None),
            C::Guid(None),
            C::Binary(None),
            C::Numeric(None),
            C::Xml(None),
            C::Date(None),
            C::Time(None),
            C::DateTime(None),
            C::SmallDateTime(None),
            C::DateTime2(None),
            C::DateTimeOffset(None),
        ] {
            assert_eq!(tiberius_cell(&cell, T::Null), None);
        }
        for ty in [T::Money, T::Money4] {
            for (v, expected) in
                [(0.0, "0.0000"), (1.2, "1.2000"), (-123.4567, "-123.4567"), (214748.3647, "214748.3647")]
            {
                assert_eq!(tiberius_cell(&C::F64(Some(v)), ty).as_deref(), Some(expected));
            }
            assert_eq!(tiberius_cell(&C::F64(None), ty), None);
        }
    }

    fn prop(name: &str, value: &str) -> DbProperty {
        DbProperty { name: name.into(), value: value.into() }
    }

    #[test]
    fn postgres_options_become_runtime_parameters() {
        let s = DataSource {
            driver: DbDriver::Postgres,
            read_only: true,
            options: DbOptions {
                default_schema: "app, public".into(),
                time_zone: "Europe/Paris".into(),
                startup_script: "SET work_mem = '64MB'; select 1;".into(),
                ..Default::default()
            },
            properties: vec![prop("statement_timeout", "5s"), prop("application_name", "me"), prop("", "x")],
            ..Default::default()
        };
        let setup = Setup::of(&s).unwrap();
        let names: Vec<_> = setup.params.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["default_transaction_read_only", "search_path", "TimeZone", "statement_timeout"]);
        assert_eq!(setup.application_name.as_deref(), Some("me"));
        assert_eq!(setup.statements, ["SET work_mem = '64MB'", "select 1"]);
    }

    #[test]
    fn mysql_properties_become_session_variables() {
        let s = DataSource {
            driver: DbDriver::Mysql,
            read_only: true,
            properties: vec![
                prop("sql_mode", "ANSI_QUOTES"),
                prop("max_execution_time", "1000"),
                prop("charset", "utf8mb4"),
                prop("connect-timeout", "5"),
            ],
            ..Default::default()
        };
        let setup = Setup::of(&s).unwrap();
        assert_eq!(
            setup.statements,
            [
                "SET SESSION TRANSACTION READ ONLY",
                "SET SESSION sql_mode = 'ANSI_QUOTES'",
                "SET SESSION max_execution_time = 1000"
            ]
        );
        assert_eq!(setup.charset.as_deref(), Some("utf8mb4"));
        assert_eq!(setup.connect_timeout, Some(Duration::from_secs(5)));
        assert!(Setup::of(&DataSource { properties: vec![prop("a; drop", "1")], ..s.clone() }).is_err());
        assert!(Setup::of(&DataSource { properties: vec![prop("connect_timeout", "soon")], ..s }).is_err());
    }

    #[test]
    fn sqlite_properties_become_pragmas() {
        let s = DataSource { properties: vec![prop("cache_size", "-2000")], ..Default::default() };
        assert_eq!(Setup::of(&s).unwrap().params, [("cache_size".to_owned(), "-2000".to_owned())]);
    }

    #[test]
    fn only_bytes_typed_values_are_binary() {
        // (type name as the driver reports it, the driver reads it as text, binary)
        let cases = [
            ("VARBINARY", false, true),
            ("BLOB", false, true),
            ("LONGBLOB", false, true),
            ("BINARY", false, true),
            // MySQL catalog columns (`utf8mb3_bin`) and `utf8mb4_bin` text: flagged BINARY.
            ("VARBINARY", true, false),
            ("BLOB", true, false),
            ("VARCHAR", true, false),
            ("TEXT", true, false),
            ("BYTEA", false, false),
            ("INT", false, false),
        ];
        for (name, text, binary) in cases {
            assert_eq!(binary_type(name, text), binary, "{name} text={text}");
        }
    }

    #[test]
    fn catalog_bytes_are_names_and_grid_bytes_are_hex() {
        // `oi_数据` as MySQL sends it under `character_set_results = binary`.
        let name = "oi_数据".as_bytes().to_vec();
        assert_eq!(bytes_text(name.clone(), Cells::Text), "oi_数据");
        assert_eq!(bytes_text(name, Cells::Display), "oi_数据");
        assert_eq!(bytes_text(vec![0xCA, 0xFE], Cells::Display), "0xCAFE");
        assert_eq!(bytes_text(vec![b'a', 0xFF], Cells::Text), "a\u{FFFD}");
    }
}
