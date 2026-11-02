import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { isInsideRepository, resolveOutputDirectory } from "./create-owner-invitation.mjs";

for (const [name, paths, repository] of [
  ["POSIX", path.posix, "/repo"],
  ["Windows", path.win32, "C:\\repo"],
]) {
  test(`${name} rejects repository paths and permits outside directories`, () => {
    for (const destination of [
      repository,
      paths.join(repository, "private"),
      paths.join(repository, "private", "nested"),
      paths.join(repository, "..", "repo", "private"),
    ]) {
      assert.equal(isInsideRepository(repository, paths.resolve(destination), paths), true);
    }
    for (const destination of [
      paths.dirname(repository),
      repository + "-private",
      paths.join(paths.dirname(repository), "private"),
      paths.join(repository, "..", "private"),
    ]) {
      assert.equal(isInsideRepository(repository, paths.resolve(destination), paths), false);
    }
  });
}

test("Windows comparisons handle casing, alternate separators, and different drives", () => {
  for (const destination of ["c:\\REPO", "c:\\REPO\\private", "C:/repo/private"]) {
    assert.equal(isInsideRepository("C:\\repo", destination, path.win32), true);
  }
  assert.equal(isInsideRepository("C:\\repo", "D:\\private", path.win32), false);
});

test("CLI rejects repository output before writing and creates private files outside", (t) => {
  const repository = path.resolve(import.meta.dirname, "..");
  const script = path.join(repository, "scripts/create-owner-invitation.mjs");
  const inside = path.join(repository, `invitation-test-${process.pid}`);
  const run = (destination) =>
    spawnSync(process.execPath, [script, destination], { encoding: "utf8" });
  const rejected = run(inside);
  assert.notEqual(rejected.status, 0);
  assert.match(rejected.stderr, /outside the repository/);
  assert.equal(existsSync(inside), false);

  const outside = mkdtempSync(path.join(tmpdir(), "owner-invitation-test-"));
  t.after(() => rmSync(outside, { recursive: true, force: true }));
  assert.equal(run(outside).status, 0);
  const code = readFileSync(path.join(outside, "owner-invitation.txt"), "utf8").trim();
  const config = readFileSync(path.join(outside, "owner-auth.env"), "utf8");
  assert.match(code, /^[A-Za-z0-9_-]{43}$/);
  assert.equal(
    config,
    `WF_ACCOUNT_AUTH=true\nWF_SIGNUP_INVITE_HASH=${createHash("sha256").update(code).digest("hex")}\n`,
  );
  assert.notEqual(run(outside).status, 0);
  assert.equal(readFileSync(path.join(outside, "owner-invitation.txt"), "utf8").trim(), code);
});

test("CLI requires a destination instead of writing invitation files", () => {
  const script = path.resolve(import.meta.dirname, "create-owner-invitation.mjs");
  const result = spawnSync(process.execPath, [script], { encoding: "utf8" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Usage: node scripts\/create-owner-invitation.mjs/);
});

test("CLI rejects outside symlinks into the repository before creating files or directories", (t) => {
  const root = mkdtempSync(path.join(tmpdir(), "owner-invitation-symlink-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const repository = path.join(root, "repository");
  mkdirSync(path.join(repository, "scripts"), { recursive: true });
  const script = path.join(repository, "scripts/create-owner-invitation.mjs");
  cpSync(path.join(import.meta.dirname, "create-owner-invitation.mjs"), script);
  const alias = path.join(root, "outside-alias");
  symlinkSync(repository, alias, process.platform === "win32" ? "junction" : "dir");
  for (const destination of [alias, path.join(alias, "new", "private")]) {
    const result = spawnSync(process.execPath, [script, destination], { encoding: "utf8" });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /outside the repository/);
    assert.equal(existsSync(path.join(repository, "owner-invitation.txt")), false);
    assert.equal(existsSync(path.join(repository, "owner-auth.env")), false);
    assert.equal(existsSync(path.join(repository, "new")), false);
  }
});

test("outside directory aliases resolve to a private real destination", (t) => {
  const root = mkdtempSync(path.join(tmpdir(), "owner-invitation-realpath-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const target = path.join(root, "private");
  mkdirSync(target);
  const alias = path.join(root, "alias");
  symlinkSync(target, alias, process.platform === "win32" ? "junction" : "dir");
  assert.equal(
    resolveOutputDirectory(path.join(alias, "new", "nested")),
    path.join(target, "new", "nested"),
  );
  const script = path.join(import.meta.dirname, "create-owner-invitation.mjs");
  const result = spawnSync(process.execPath, [script, path.join(alias, "new", "nested")], {
    encoding: "utf8",
  });
  assert.equal(result.status, 0);
  assert.equal(existsSync(path.join(target, "new", "nested", "owner-invitation.txt")), true);
  assert.equal(existsSync(path.join(target, "new", "nested", "owner-auth.env")), true);
});

test("Git and build contexts exclude invitation files at the root and in subdirectories", () => {
  const repository = path.resolve(import.meta.dirname, "..");
  const names = ["owner-invitation.txt", "owner-auth.env"];
  const paths = names.flatMap((name) => [name, `private/${name}`]);
  const ignored = spawnSync("git", ["check-ignore", "--no-index", ...paths], {
    cwd: repository,
    encoding: "utf8",
  });
  assert.equal(ignored.status, 0);
  assert.deepEqual(ignored.stdout.trim().split(/\r?\n/), paths);
  for (const file of [".dockerignore", ".gcloudignore"]) {
    const patterns = readFileSync(path.join(repository, file), "utf8").split(/\r?\n/);
    for (const name of names) {
      assert.ok(patterns.includes(name), `${file} must exclude root ${name}`);
      assert.ok(patterns.includes(`**/${name}`), `${file} must exclude nested ${name}`);
    }
  }
});
