import { describe, expect, it } from "vitest";
import { detectOs } from "../../src/lib/os";

const WINDOWS = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const IPAD_AS_MAC = MAC; // iPadOS 13+ sends the same string as Safari on a Mac
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
const LINUX = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";
const UBUNTU = "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";
const ANDROID = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36";
const CHROMEOS = "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

describe("detectOs from the user agent", () => {
  it("finds Windows", () => {
    expect(detectOs(WINDOWS)).toBe("windows");
  });

  it("finds macOS", () => {
    expect(detectOs(MAC)).toBe("macos");
  });

  it("treats an iPad that reports itself as a Mac as macOS, since the string is the same", () => {
    expect(detectOs(IPAD_AS_MAC)).toBe("macos");
  });

  it("finds Linux, with or without a distribution name", () => {
    expect(detectOs(LINUX)).toBe("linux");
    expect(detectOs(UBUNTU)).toBe("linux");
  });

  it("returns null for Android, whose user agent also says Linux", () => {
    expect(detectOs(ANDROID)).toBeNull();
  });

  it("returns null for ChromeOS, whose user agent also says X11 and Linux", () => {
    expect(detectOs(CHROMEOS)).toBeNull();
  });

  it("returns null for an iPhone, whose user agent says Mac OS X", () => {
    expect(detectOs(IPHONE)).toBeNull();
  });

  it("is not fooled by a word that contains cros, such as Microsoft", () => {
    expect(detectOs(`${WINDOWS} Microsoft Office/16.0`)).toBe("windows");
    expect(detectOs("Mozilla/5.0 (Windows NT 10.0; Microsoft Outlook 16.0) Linux-free")).toBe("windows");
  });

  it("returns null for an iPad that says so, since its string also says Mac OS X", () => {
    const iPad = "Mozilla/5.0 (iPad; CPU OS 12_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/12.1.2 Mobile/15E148 Safari/604.1";
    expect(detectOs(iPad)).toBeNull();
  });

  it("returns null for an empty or unknown user agent", () => {
    expect(detectOs("")).toBeNull();
    expect(detectOs("curl/8.5.0")).toBeNull();
  });
});

describe("detectOs with the userAgentData platform hint", () => {
  it("wins over the user agent", () => {
    expect(detectOs(LINUX, "Windows")).toBe("windows");
    expect(detectOs(WINDOWS, "macOS")).toBe("macos");
    expect(detectOs(MAC, "Linux")).toBe("linux");
  });

  it("returns null when the hint names a system without a download, even if the string looks like Linux", () => {
    expect(detectOs(LINUX, "Android")).toBeNull();
    expect(detectOs(LINUX, "Chrome OS")).toBeNull();
  });

  it("is ignored when it is empty or missing, then the user agent decides", () => {
    expect(detectOs(WINDOWS, "")).toBe("windows");
    expect(detectOs(WINDOWS, null)).toBe("windows");
    expect(detectOs(WINDOWS, undefined)).toBe("windows");
  });

  it("matches the hint without caring about case", () => {
    expect(detectOs("", "windows")).toBe("windows");
    expect(detectOs("", "MACOS")).toBe("macos");
  });
});
