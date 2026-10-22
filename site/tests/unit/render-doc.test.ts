import { describe, expect, it } from "vitest";
import { DOCS, routesBySource } from "../../src/lib/docs";
import { renderDoc } from "../../src/lib/render-doc";

const routes = routesBySource(DOCS);

describe("renderDoc", () => {
  it("takes the page title from the first top-level heading", async () => {
    const { title } = await renderDoc("Intro\n\n# Verify *your* downloads\n\n# Another\n", "docs/verify-downloads.md", routes);
    expect(title).toBe("Verify your downloads");
  });

  it("has no title when there is no top-level heading", async () => {
    const { title } = await renderDoc("## Only a section\n", "docs/linux.md", routes);
    expect(title).toBeUndefined();
  });

  it("takes a summary from the first paragraph, as plain text of at most 160 characters", async () => {
    const { summary } = await renderDoc("# T\n\nThe app is **free** and [open](verify-downloads.md). It's `small`.\n\nSecond paragraph.\n", "docs/linux.md", routes);
    expect(summary).toBe("The app is free and open. It's small.");
    const long = await renderDoc(`# T\n\n${"word ".repeat(80)}\n`, "docs/linux.md", routes);
    expect(long.summary!.length).toBeLessThanOrEqual(160);
    expect(long.summary!.endsWith("...")).toBe(true);
  });

  it("decodes every entity in the summary, numeric ones included", async () => {
    const { summary } = await renderDoc(
      "# T\n\n`Repeat-That_<version>_x64-setup.exe` is the installer, 5 > 3 & \"quoted\" 'text'.\n",
      "docs/install-windows.md",
      routes,
    );
    expect(summary).toBe("Repeat-That_<version>_x64-setup.exe is the installer, 5 > 3 & \"quoted\" 'text'.");
  });

  it("has no summary when the doc has no paragraph", async () => {
    expect((await renderDoc("# T\n", "docs/linux.md", routes)).summary).toBeUndefined();
  });

  it("rewrites relative links and keeps the anchors", async () => {
    const { html } = await renderDoc(
      "# T\n\n[a](verify-downloads.md#windows) [b](../README.md#using-it) [c](https://example.org)\n",
      "docs/linux.md",
      routes,
    );
    expect(html).toContain('href="/verify/#windows"');
    expect(html).toContain('href="https://github.com/Roboroads/repeat-that/blob/main/README.md#using-it"');
    expect(html).toContain('href="https://example.org"');
  });

  it("gives headings the same ids GitHub does, so anchors from the repository still work", async () => {
    const { html } = await renderDoc("# T\n\n## Audio capture permission\n\n## Build from source\n", "docs/macos.md", routes);
    expect(html).toContain('id="audio-capture-permission"');
    expect(html).toContain('id="build-from-source"');
  });

  it("renders tables and leaves quotes straight", async () => {
    const { html } = await renderDoc("# T\n\n| a | b |\n|---|---|\n| 1 | don't |\n", "docs/linux.md", routes);
    expect(html).toContain("<table>");
    expect(html).toContain("don't");
  });

  it("does not highlight code, it keeps a plain pre block", async () => {
    const { html } = await renderDoc("# T\n\n```sh\nsudo apt install ./x.deb\n```\n", "docs/linux.md", routes);
    expect(html).toContain('<pre><code class="language-sh">');
  });
});
