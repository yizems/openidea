import { describe, expect, it } from "vitest";
import { numberHeadings, replacement } from "./numbering";

describe("numberHeadings", () => {
  it("numbers every level and restarts deeper counters", () => {
    const out = numberHeadings("# A\n\n## B\n\n## C\n\n### D\n\n# E\n\n## F\n");
    expect(out).toBe("# 1. A\n\n## 1.1 B\n\n## 1.2 C\n\n### 1.2.1 D\n\n# 2. E\n\n## 2.1 F\n");
  });

  it("is idempotent and replaces stale numbers", () => {
    const once = numberHeadings("# A\n## B\n## C\n");
    expect(numberHeadings(once)).toBe(once);
    expect(numberHeadings("# 3. A\n## 9.9 B\n")).toBe("# 1. A\n## 1.1 B\n");
  });

  it("keeps titles that merely start with a number", () => {
    expect(numberHeadings("# 2024 plan\n")).toBe("# 1. 2024 plan\n");
    expect(numberHeadings("# 1. 2024 plan\n")).toBe("# 1. 2024 plan\n");
  });

  it("skips code fences, front matter and non-headings", () => {
    const text = "---\ntitle: x\n# not\n---\n# A\n```md\n# code\n```\n#hash\n    # indented\n";
    expect(numberHeadings(text)).toBe("---\ntitle: x\n# not\n---\n# 1. A\n```md\n# code\n```\n#hash\n    # indented\n");
  });

  it("starts at the given level and fills skipped levels", () => {
    expect(numberHeadings("# Title\n## A\n#### B\n", 2)).toBe("# Title\n## 1. A\n#### 1.1.1 B\n");
  });
});

describe("replacement", () => {
  it("narrows to the changed middle", () => {
    expect(replacement("abcXYZdef", "abcQdef")).toEqual([{ from: 3, to: 6, insert: "Q" }]);
    expect(replacement("same", "same")).toEqual([]);
  });
});
