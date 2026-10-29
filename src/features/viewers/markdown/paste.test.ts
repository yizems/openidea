import { describe, expect, it } from "vitest";
import { assetDirectory, baseName, markdownLink, pastedImageName, relativeLink } from "./paste";

describe("assetDirectory", () => {
  it("resolves the folder against the Markdown file's folder", () => {
    expect(assetDirectory("/p/docs/a.md", "./assets")).toBe("/p/docs/assets");
    expect(assetDirectory("/p/docs/a.md", "../img")).toBe("/p/img");
    expect(assetDirectory("C:/p/docs/a.md", "assets/")).toBe("C:/p/docs/assets");
  });

  it("falls back to ./assets and keeps absolute folders", () => {
    expect(assetDirectory("/p/a.md", "  ")).toBe("/p/assets");
    expect(assetDirectory("/p/a.md", "/shared/pics/")).toBe("/shared/pics");
    expect(assetDirectory("C:/p/a.md", "D:\\pics")).toBe("D:/pics");
  });
});

describe("pastedImageName", () => {
  const now = new Date(2026, 0, 2, 3, 4, 5);

  it("names image data by prefix, time and type", () => {
    expect(pastedImageName("image", "image/png", now)).toBe("image-20260102-030405.png");
    expect(pastedImageName("shot", "image/jpeg", now)).toBe("shot-20260102-030405.jpg");
    expect(pastedImageName("", "image/unknown", now)).toBe("image-20260102-030405.png");
    expect(pastedImageName("my shot/1", "image/webp", now)).toBe("my-shot-1-20260102-030405.webp");
  });
});

describe("relativeLink", () => {
  it("links below, above and beside the Markdown file", () => {
    expect(relativeLink("/p/docs/a.md", "/p/docs/assets/x.png")).toBe("./assets/x.png");
    expect(relativeLink("/p/docs/a.md", "/p/img/x.png")).toBe("../img/x.png");
    expect(relativeLink("/p/a.md", "/p/x.png")).toBe("./x.png");
    expect(relativeLink("/a.md", "/assets/x.png")).toBe("./assets/x.png");
  });

  it("encodes characters that break Markdown links", () => {
    expect(relativeLink("/p/a.md", "/p/assets/my pic (1).png")).toBe("./assets/my%20pic%20%281%29.png");
    expect(relativeLink("/p/a.md", "/p/资料/图.png")).toBe(
      `./${encodeURIComponent("资料")}/${encodeURIComponent("图")}.png`,
    );
  });

  it("handles Windows drives", () => {
    expect(relativeLink("C:/p/a.md", "C:/p/assets/x.png")).toBe("./assets/x.png");
    expect(relativeLink("c:/p/a.md", "C:/p/assets/x.png")).toBe("./assets/x.png");
    expect(relativeLink("C:/p/a.md", "D:/pics/x.png")).toBe("D:/pics/x.png");
  });
});

describe("markdownLink", () => {
  it("embeds the saved image", () => {
    expect(markdownLink("/p/a.md", "/p/assets/image-1.png")).toBe("![image-1](./assets/image-1.png)");
    expect(markdownLink("/p/a.md", "/p/assets/a[1].png")).toBe("![a\\[1\\]](./assets/a%5B1%5D.png)");
  });
});

describe("baseName", () => {
  it("keeps the last path component only", () => {
    expect(baseName("/p/docs/report.pdf")).toBe("report.pdf");
    expect(baseName("C:\\p\\docs\\my report.pdf")).toBe("my report.pdf");
    expect(baseName("/p/folder/")).toBe("folder");
  });
});
