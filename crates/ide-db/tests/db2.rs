//! Live Db2 checks, skipped unless OPENIDEA_TEST_DB2=host:port:database:user:password:driver
//! names a throwaway database and an ODBC driver registered with the system's driver
//! manager (`odbcinst -q -d`; IBM's Db2 ODBC/CLI driver works).

use ide_core::CancellationToken;
use ide_db::{DataSource, DbDriver, Held, Pool};

#[tokio::test]
async fn db2_values_paging_transactions_and_cancellation() {
    let Ok(server) = std::env::var("OPENIDEA_TEST_DB2") else { return };
    let mut parts = server.splitn(6, ':');
    let source = DataSource {
        driver: DbDriver::Db2,
        host: parts.next().unwrap().into(),
        port: Some(parts.next().unwrap().parse().unwrap()),
        database: parts.next().unwrap().into(),
        user: parts.next().unwrap().into(),
        ..Default::default()
    };
    let password = parts.next();
    let odbc_driver = parts.next().unwrap_or_default().to_owned();
    let source = DataSource { odbc_driver, ..source };
    let pool = Pool::connect(&source, password, None).await.unwrap();
    let cancel = CancellationToken::new();
    let mut held = Held::acquire(&pool).await.unwrap();
    let table = format!("oi_pr_values_{}", std::process::id());
    held.command(&format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY, n DECIMAL(10, 2), t VARCHAR(20), d DATE)"))
        .await
        .unwrap();
    held.command(&format!("INSERT INTO {table} VALUES (1, 12.34, 'hello', '2024-02-29')")).await.unwrap();
    held.command(&format!("INSERT INTO {table} (id) VALUES (2)")).await.unwrap();
    held.command("COMMIT").await.unwrap();
    let r = held.execute(&pool, &format!("SELECT * FROM {table} ORDER BY id"), 1, &cancel).await.unwrap();
    assert!(r.has_more);
    assert_eq!(r.rows[0], ["1", "12.34", "hello", "2024-02-29"].map(|s| Some(s.to_owned())));
    let r = held.execute(&pool, &format!("SELECT * FROM {table} WHERE id = 2"), 5, &cancel).await.unwrap();
    assert!(!r.has_more);
    assert!(r.rows[0][1..].iter().all(Option::is_none));
    held.command(&format!("DROP TABLE {table}")).await.unwrap();
    drop(held);

    let table = format!("oi_pr_{}", std::process::id());
    pool.execute(&format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY, v INTEGER)"), 1, &cancel).await.unwrap();
    pool.transaction(&[(format!("INSERT INTO {table} VALUES (1, 10)"), true)]).await.unwrap();
    assert!(
        pool.transaction(&[
            (format!("UPDATE {table} SET v = 99 WHERE id = 1"), true),
            (format!("UPDATE {table} SET v = 30 WHERE id = 999"), true),
        ])
        .await
        .is_err()
    );
    assert_eq!(pool.rows(&format!("SELECT v FROM {table}"), 1).await.unwrap()[0][0].as_deref(), Some("10"));
    pool.execute(&format!("DROP TABLE {table}"), 1, &cancel).await.unwrap();
}
