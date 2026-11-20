/**
 * comP が MAGATAMA の起動登録を管理するための判断（src/config/magatama.ts）。
 * Owned by the designer role; implementers must not edit.
 * 計画: tasks/magatama_sync_plan.md（T4: ユーザーの設定ファイルを書き換える判断で、版の解釈と管理/非管理の判定が密）
 */
import { expect } from "chai";
import {
  MAGATAMA_MIN,
  MAGATAMA_PIN,
  assessInstalledMagatama,
  compareVersions,
  managedEntry,
  parseMagatamaVersionOutput,
  parseManagedEntry,
  parseVersion,
  planMagatamaEntry,
  versionProbeFor,
} from "../config/magatama";

const marker = { COMP_MANAGED_BY: "comP" };
const managed = (version: string, extra: Record<string, unknown> = {}): Record<string, unknown> => ({
  command: "uvx",
  args: [`magatama@${version}`, "serve"],
  env: { ...marker },
  ...extra,
});

describe("magatama: constants", () => {
  it("pins the MAGATAMA release comP was verified with", () => {
    expect(MAGATAMA_PIN).to.equal("0.7.2");
    expect(MAGATAMA_MIN).to.equal("0.7.2");
  });
});

describe("magatama: parseVersion / compareVersions", () => {
  const valid: Array<[string, [number, number, number]]> = [
    ["0.7.2", [0, 7, 2]],
    ["10.0.13", [10, 0, 13]],
    ["0.0.0", [0, 0, 0]],
  ];
  for (const [input, expected] of valid) {
    it(`parses ${input}`, () => expect(parseVersion(input)).to.deep.equal(expected));
  }
  for (const input of ["0.7", "0.7.2rc1", "v0.7.2", " 0.7.2", "0.7.2 ", "", "latest", "0.7.x", "1.2.3.4", "-1.0.0"]) {
    it(`rejects ${JSON.stringify(input)}`, () => expect(parseVersion(input)).to.equal(null));
  }

  const order: Array<[string, string, -1 | 0 | 1]> = [
    ["0.7.1", "0.7.2", -1],
    ["0.7.2", "0.7.2", 0],
    ["0.8.0", "0.7.2", 1],
    ["0.7.10", "0.7.9", 1], // 数値で比べる（文字列比較だと逆になる）
    ["1.0.0", "0.99.99", 1],
  ];
  for (const [a, b, expected] of order) {
    it(`compares ${a} with ${b}`, () => expect(compareVersions(a, b)).to.equal(expected));
  }
  it("returns null when either side is unparseable", () => {
    expect(compareVersions("0.7", "0.7.2")).to.equal(null);
    expect(compareVersions("0.7.2", "latest")).to.equal(null);
  });
});

describe("magatama: managedEntry / parseManagedEntry", () => {
  it("writes uvx with the pinned package and comP's marker", () => {
    expect(managedEntry("0.7.2")).to.deep.equal({
      command: "uvx",
      args: ["magatama@0.7.2", "serve"],
      env: { COMP_MANAGED_BY: "comP" },
    });
  });

  it("recognises its own entry", () => {
    expect(parseManagedEntry(managedEntry("0.7.2"))).to.deep.equal({ managed: true, version: "0.7.2" });
  });

  const managedVariants: Array<[string, unknown, string]> = [
    ["absolute Windows path to uvx.exe", managed("0.7.1", { command: "C:\\Users\\u\\.local\\bin\\uvx.exe" }), "0.7.1"],
    ["POSIX path", managed("0.7.1", { command: "/home/u/.local/bin/uvx" }), "0.7.1"],
    ["upper-case command", managed("0.7.1", { command: "UVX.EXE" }), "0.7.1"],
    ["extra args before the package", { command: "uvx", args: ["--quiet", "magatama@0.7.1", "serve"], env: marker }, "0.7.1"],
    ["unparseable version is still ours", managed("latest"), "latest"],
    ["other env keys kept", managed("0.7.1", { env: { ...marker, MAGATAMA_LOG: "debug" } }), "0.7.1"],
  ];
  for (const [name, entry, version] of managedVariants) {
    it(`treats as managed: ${name}`, () => {
      expect(parseManagedEntry(entry)).to.deep.equal({ managed: true, version });
    });
  }

  const userVariants: Array<[string, unknown]> = [
    ["no marker (user pinned uvx themselves)", { command: "uvx", args: ["magatama@0.7.1", "serve"] }],
    ["marker with another value", managed("0.7.1", { env: { COMP_MANAGED_BY: "someone" } })],
    ["env is not an object", managed("0.7.1", { env: "COMP_MANAGED_BY=comP" })],
    ["plain magatama command", { command: "magatama", args: ["serve"], env: marker }],
    ["uv run --directory", { command: "uv", args: ["run", "--directory", "C:\\M", "magatama", "serve"], env: marker }],
    ["no serve", { command: "uvx", args: ["magatama@0.7.1"], env: marker }],
    ["two magatama@ packages", { command: "uvx", args: ["magatama@0.7.1", "magatama@0.7.2", "serve"], env: marker }],
    ["args not an array", { command: "uvx", args: "magatama@0.7.1 serve", env: marker }],
    ["args with a non-string", { command: "uvx", args: ["magatama@0.7.1", 3, "serve"], env: marker }],
    ["command not a string", { command: ["uvx"], args: ["magatama@0.7.1", "serve"], env: marker }],
    ["uvx-like name", managed("0.7.1", { command: "uvxx" })],
    ["null", null],
    ["array", []],
    ["string", "uvx magatama@0.7.1 serve"],
    ["undefined", undefined],
  ];
  for (const [name, entry] of userVariants) {
    it(`treats as not managed: ${name}`, () => {
      expect(parseManagedEntry(entry)).to.deep.equal({ managed: false });
    });
  }
});

describe("magatama: planMagatamaEntry", () => {
  const on = { manage: true, uvAvailable: true };
  const off = { manage: false, uvAvailable: true };

  it("adds nothing when the user has not asked comP to manage MAGATAMA", () => {
    expect(planMagatamaEntry(undefined, "0.7.2", off)).to.deep.equal({ action: "keep", reason: "not-managed-yet" });
  });
  it("does not write an entry that cannot start without uv", () => {
    expect(planMagatamaEntry(undefined, "0.7.2", { manage: true, uvAvailable: false })).to.deep.equal({
      action: "keep",
      reason: "uv-missing",
    });
  });
  it("adds the managed entry when asked and uv is there", () => {
    expect(planMagatamaEntry(undefined, "0.7.2", on)).to.deep.equal({ action: "add", entry: managedEntry("0.7.2") });
  });

  for (const opts of [on, off, { manage: true, uvAvailable: false }]) {
    it(`never touches the user's own entry (manage=${opts.manage}, uv=${opts.uvAvailable})`, () => {
      const user = { command: "magatama", args: ["serve"] };
      expect(planMagatamaEntry(user, "0.7.2", opts)).to.deep.equal({ action: "keep", reason: "user-managed" });
    });
  }

  it("upgrades an older managed entry, keeping the user's extra keys", () => {
    const existing = managed("0.7.1", { env: { ...marker, MAGATAMA_LOG: "debug" }, timeout: 60 });
    const before = JSON.parse(JSON.stringify(existing));
    const plan = planMagatamaEntry(existing, "0.7.2", off);
    expect(plan).to.deep.equal({
      action: "update",
      entry: {
        command: "uvx",
        args: ["magatama@0.7.2", "serve"],
        env: { ...marker, MAGATAMA_LOG: "debug" },
        timeout: 60,
      },
    });
    expect(existing).to.deep.equal(before); // 入力を書き換えない
  });
  it("keeps the position of the package among other args", () => {
    const existing = { command: "uvx", args: ["--quiet", "magatama@0.7.1", "serve"], env: marker };
    const plan = planMagatamaEntry(existing, "0.7.2", off);
    expect(plan).to.deep.equal({
      action: "update",
      entry: { command: "uvx", args: ["--quiet", "magatama@0.7.2", "serve"], env: marker },
    });
  });
  it("keeps an entry already at the pin", () => {
    expect(planMagatamaEntry(managed("0.7.2"), "0.7.2", on)).to.deep.equal({ action: "keep", reason: "up-to-date" });
  });
  it("never downgrades an entry written by a newer comP", () => {
    expect(planMagatamaEntry(managed("0.8.0"), "0.7.2", on)).to.deep.equal({ action: "keep", reason: "newer" });
    expect(planMagatamaEntry(managed("0.7.10"), "0.7.9", on)).to.deep.equal({ action: "keep", reason: "newer" });
  });
  it("leaves a managed entry with an unparseable version alone", () => {
    expect(planMagatamaEntry(managed("latest"), "0.7.2", on)).to.deep.equal({
      action: "keep",
      reason: "unparseable-version",
    });
  });
});

describe("magatama: planMagatamaEntry invariants (seeded random inputs)", () => {
  // 依存を足さずに、種付き乱数で 500 件の入力を作る（mulberry32）。失敗したら seed と件番号で再現できる。
  const seed = 20261121;
  const rng = (() => {
    let a = seed >>> 0;
    return () => {
      a = (a + 0x6d2b79f5) >>> 0;
      let t = a;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  })();
  const pick = <T>(xs: T[]): T => xs[Math.floor(rng() * xs.length)];
  const versions = ["0.7.1", "0.7.2", "0.7.10", "0.8.0", "1.0.0", "0.0.9", "latest", "", "0.7"];
  const commands = ["uvx", "UVX.exe", "C:\\x\\uvx.exe", "/u/bin/uvx", "magatama", "uv", "uvxx", 3, undefined];
  const envs = [marker, { ...marker, A: "1" }, { COMP_MANAGED_BY: "x" }, {}, undefined, "comP", null];
  const genArgs = (): unknown => {
    const shape = pick(["normal", "extra", "noserve", "double", "notarray", "nonstring"]);
    const v = pick(versions);
    switch (shape) {
      case "normal":
        return [`magatama@${v}`, "serve"];
      case "extra":
        return ["--quiet", `magatama@${v}`, "serve"];
      case "noserve":
        return [`magatama@${v}`];
      case "double":
        return [`magatama@${v}`, `magatama@${pick(versions)}`, "serve"];
      case "notarray":
        return `magatama@${v} serve`;
      default:
        return [`magatama@${v}`, 1, "serve"];
    }
  };
  const genEntry = (): unknown => {
    const kind = rng();
    if (kind < 0.05) return undefined;
    if (kind < 0.1) return pick([null, [], "x", 0]);
    const entry: Record<string, unknown> = { command: pick(commands), args: genArgs() };
    const env = pick(envs);
    if (env !== undefined) entry.env = env;
    if (rng() < 0.3) entry.timeout = 30;
    return entry;
  };

  it("holds for 500 generated entries", () => {
    for (let i = 0; i < 500; i++) {
      const existing = genEntry();
      const pin = pick(["0.7.2", "0.7.9", "0.8.0"]);
      const opts = { manage: rng() < 0.5, uvAvailable: rng() < 0.5 };
      const snapshot = JSON.stringify(existing);
      const plan = planMagatamaEntry(existing, pin, opts);
      const ctx = `seed=${seed} case=${i} existing=${snapshot} pin=${pin} opts=${JSON.stringify(opts)}`;
      const parsed = parseManagedEntry(existing);

      // 1. ユーザー管理の登録（存在するもの）は、何があっても keep(user-managed)
      if (existing !== undefined && !parsed.managed) {
        expect(plan, ctx).to.deep.equal({ action: "keep", reason: "user-managed" });
      }
      // 2. add は登録がないときだけで、管理を任されて uv があるときだけ
      if (plan.action === "add") {
        expect(existing, ctx).to.equal(undefined);
        expect(opts.manage && opts.uvAvailable, ctx).to.equal(true);
      }
      // 3. update の結果は管理登録で版 = pin。args 以外のキーは元と同じ。版を下げない
      if (plan.action === "update") {
        expect(parsed.managed, ctx).to.equal(true);
        expect(parseManagedEntry(plan.entry), ctx).to.deep.equal({ managed: true, version: pin });
        const before = existing as Record<string, unknown>;
        for (const key of Object.keys(before)) {
          if (key !== "args") expect(plan.entry[key], `${ctx} key=${key}`).to.deep.equal(before[key]);
        }
        expect(Object.keys(plan.entry).sort(), ctx).to.deep.equal(Object.keys(before).sort());
        if (parsed.managed) expect(compareVersions(parsed.version, pin), ctx).to.equal(-1);
      }
      // 4. 入力を書き換えない
      expect(JSON.stringify(existing), ctx).to.equal(snapshot);
    }
  });
});

describe("magatama: version output and assessment", () => {
  const outputs: Array<[string | null | undefined, string | null]> = [
    ["MAGATAMA, version 0.7.2", "0.7.2"],
    ["MAGATAMA, version 0.7.2\n", "0.7.2"],
    ["magatama, version 0.6.0\r\n", "0.6.0"],
    ["MAGATAMA, version 0.7.2rc1", null],
    ["", null],
    [null, null],
    [undefined, null],
    ["error: unrecognized option", null],
    ["Python 3.13.1", null], // 別の版の行を拾わない
  ];
  for (const [output, expected] of outputs) {
    it(`reads ${JSON.stringify(output)}`, () => expect(parseMagatamaVersionOutput(output)).to.equal(expected));
  }

  const assessments: Array<[string | null, string, "ok" | "outdated" | "unknown"]> = [
    ["MAGATAMA, version 0.7.1", "0.7.2", "outdated"],
    ["MAGATAMA, version 0.7.2", "0.7.2", "ok"],
    ["MAGATAMA, version 0.8.0", "0.7.2", "ok"],
    ["MAGATAMA, version 0.7.10", "0.7.9", "ok"],
    [null, "0.7.2", "unknown"], // 判定できないときは通知しない
    ["garbage", "0.7.2", "unknown"],
  ];
  for (const [output, min, expected] of assessments) {
    it(`assesses ${JSON.stringify(output)} against ${min}`, () => {
      expect(assessInstalledMagatama(output, min)).to.equal(expected);
    });
  }
});

describe("magatama: versionProbeFor", () => {
  it("never runs a workspace config's command", () => {
    const entry = { command: "C:\\repo\\evil.exe", args: ["serve"] };
    expect(versionProbeFor(entry, "workspace")).to.deep.equal({ command: "magatama", args: ["--version"] });
  });
  it("replaces serve with --version for a home config", () => {
    expect(versionProbeFor({ command: "magatama", args: ["serve"] }, "global")).to.deep.equal({
      command: "magatama",
      args: ["--version"],
    });
    expect(
      versionProbeFor({ command: "uv", args: ["run", "--directory", "C:\\M", "magatama", "serve"] }, "global")
    ).to.deep.equal({ command: "uv", args: ["run", "--directory", "C:\\M", "magatama", "--version"] });
  });
  it("replaces only the last serve", () => {
    expect(versionProbeFor({ command: "x", args: ["serve", "magatama", "serve"] }, "global")).to.deep.equal({
      command: "x",
      args: ["serve", "magatama", "--version"],
    });
  });
  const unusable: unknown[] = [
    { command: "magatama", args: ["start"] },
    { command: "", args: ["serve"] },
    { command: 3, args: ["serve"] },
    { command: "magatama", args: "serve" },
    { command: "magatama", args: ["serve", 1] },
    null,
    "magatama serve",
  ];
  for (const entry of unusable) {
    it(`gives up on ${JSON.stringify(entry)}`, () => expect(versionProbeFor(entry, "global")).to.equal(null));
  }
});
