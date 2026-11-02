import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { isInsideRepository } from "./create-owner-invitation.mjs";

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
