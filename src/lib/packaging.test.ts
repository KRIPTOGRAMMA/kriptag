import { describe, it, expect } from "vitest";

// The guard over the metadata that ships inside the package.
//
// None of this is visible while developing: `description = "A Tauri App"` and
// `authors = ["you"]` are Tauri's template defaults, and they sat there through
// every release build, going straight into `Comment=` of the .desktop file and
// `Maintainer:` of the .deb. An empty `bundle.category` produces `Categories=`
// with nothing after it, which some launchers use to hide the entry entirely.
// The only way to notice is to install the package and look — so it is checked
// here instead.

// The options object has to be written out at every call: import.meta.glob is
// resolved at build time, so a shared constant is not something Vite can read.
const FILES = {
  ...(import.meta.glob("/package.json", { query: "?raw", import: "default", eager: true }) as Record<string, string>),
  ...(import.meta.glob("/src-tauri/tauri.conf.json", { query: "?raw", import: "default", eager: true }) as Record<string, string>),
};

const CARGO = import.meta.glob("/src-tauri/Cargo.toml", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

// LICENSE has no extension, and a bare "/LICENSE" pattern makes the glob
// crawler throw outright. A wildcard gives it something to match on, which is
// enough to tell whether the file is there — importing node:fs instead would
// need Node types the frontend tsconfig does not carry.
const LICENSE = import.meta.glob("/LICENSE*", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

function tauriConf(): Record<string, unknown> {
  return JSON.parse(FILES["/src-tauri/tauri.conf.json"]);
}

describe("метаданные пакета", () => {
  it("страж читает манифесты", () => {
    // A moved or renamed file would otherwise leave this file checking nothing.
    expect(Object.keys(FILES).length, `найдено: ${Object.keys(FILES).join(", ")}`).toBe(2);
    expect(Object.keys(CARGO).length).toBe(1);
  });

  it("в манифестах не осталось заглушек шаблона Tauri", () => {
    const cargo = CARGO["/src-tauri/Cargo.toml"];
    expect(cargo, "описание из шаблона Tauri").not.toContain('description = "A Tauri App"');
    expect(cargo, "автор из шаблона Tauri").not.toContain('authors = ["you"]');

    const pkg = JSON.parse(FILES["/package.json"]) as Record<string, string>;
    expect(pkg.description ?? "", "пустое описание в package.json").not.toBe("");
    expect(pkg.author ?? "", "нет автора в package.json").not.toBe("");
  });

  it("bundle заполнен тем, что видно в системе", () => {
    const bundle = tauriConf().bundle as Record<string, unknown>;
    // An empty Categories= line hides the entry in some launchers.
    expect(bundle.category, "не задана категория — пустой Categories= в .desktop").toBeTruthy();
    expect(bundle.shortDescription, "нет краткого описания — в deb попадёт заглушка").toBeTruthy();
    expect(bundle.longDescription, "нет полного описания — в deb будет (none)").toBeTruthy();
    expect(bundle.publisher, "нет издателя").toBeTruthy();
    // licenseFile is a declaration, not a guarantee: inspecting a built package
    // showed Tauri's deb bundler does NOT turn it into
    // usr/share/doc/<pkg>/copyright, which Debian policy requires. The key stays
    // (other bundle targets read it), but deb cannot be relied on for it.
    expect(bundle.licenseFile, "не указан файл лицензии").toBeTruthy();
  });

  it("категория — из списка, который принимает Tauri", () => {
    // "Office" looks like the obvious value and fails the build outright with
    // `invalid category`. The list is closed (Productivity, Utility, Business, …)
    // and Tauri maps it to the freedesktop category itself — Productivity comes
    // out as `Categories=Office;`. Verified on a built .deb.
    const VALID = ["Business", "DeveloperTool", "Education", "Entertainment", "Finance",
      "GraphicsAndDesign", "HealthcareAndFitness", "Lifestyle", "Medical", "Music",
      "News", "Photography", "Productivity", "Reference", "SocialNetworking",
      "Sports", "Travel", "Utility", "Video", "Weather"];
    const bundle = tauriConf().bundle as Record<string, string>;
    expect(VALID, `категория "${bundle.category}" не из списка Tauri — сборка упадёт`)
      .toContain(bundle.category);
  });

  it("лицензия заявлена одинаково везде и файл на месте", () => {
    // The declared licence and the shipped one have to agree: when they drift
    // apart the user has no answer to the question of what terms they got the
    // program under. That already happened here — the manifests said MIT while
    // no LICENSE file existed at all.
    const pkg = JSON.parse(FILES["/package.json"]) as Record<string, string>;
    expect(pkg.license).toBe("Apache-2.0");
    expect(CARGO["/src-tauri/Cargo.toml"], "лицензия не заявлена в Cargo.toml").toContain('license = "Apache-2.0"');
    // licenseFile points at ../LICENSE, so the file has to exist or the bundle
    // step fails for no visible reason.
    expect(Object.keys(LICENSE), "LICENSE отсутствует, хотя лицензия заявлена").toContain("/LICENSE");
    // That the file is the right text, not just any: Apache-2.0 is recognisable
    // by its heading, and the appendix placeholder must be filled in.
    expect(LICENSE["/LICENSE"], "в LICENSE лежит не текст Apache-2.0").toContain("Apache License");
    expect(LICENSE["/LICENSE"], "в тексте остался placeholder [yyyy]").not.toContain("[yyyy]");
  });

  it("сайдкары не отслеживаются git", async () => {
    // Sidecars are downloaded by scripts/fetch-sidecars.mjs, and src-tauri/binaries/
    // is in .gitignore — but .gitignore has no effect on a file that is already in
    // the index, and one wrapper script got in before the rule existed. It survived
    // there for months and broke CI twice in a row, in a way that reads as the
    // opposite of what it is: the fetch script finds a file of that name already
    // present, reports "already here", and skips the download. The bundler then
    // aborts on the real binary being missing.
    //
    // This has to ask git rather than the filesystem: on a developer machine the
    // directory is correctly full of binaries, and only the index tells tracked
    // apart from untracked. Hence git itself — nothing else here reaches the index.
    //
    // The import is dynamic and typed by hand because the frontend tsconfig
    // carries no @types/node and pins `types: ["vite/client"]`: a plain
    // `import ... from "node:child_process"` fails svelte-check. Widening the
    // project-wide type set for one test would hand Node APIs to every .svelte
    // file, which is precisely what that config is keeping out.
    // The specifier is assembled at runtime so that svelte-check does not try to
    // resolve it statically; vitest runs in Node and resolves it fine.
    const mod = "node:child" + "_process";
    const { execSync } = (await import(/* @vite-ignore */ mod)) as {
      execSync: (cmd: string, opts: { encoding: string }) => string;
    };
    const tracked = execSync("git ls-files src-tauri/binaries/", { encoding: "utf8" }).trim();
    expect(tracked, "сайдкар попал в индекс git — в чистом клоне он подменит скачивание").toBe("");
  });

  it("версии во всех трёх манифестах совпадают", () => {
    const pkg = JSON.parse(FILES["/package.json"]) as Record<string, string>;
    const conf = tauriConf() as Record<string, string>;
    const cargoVersion = /^version = "([^"]+)"/m.exec(CARGO["/src-tauri/Cargo.toml"])?.[1];
    expect([pkg.version, conf.version, cargoVersion]).toEqual([pkg.version, pkg.version, pkg.version]);
  });
});
