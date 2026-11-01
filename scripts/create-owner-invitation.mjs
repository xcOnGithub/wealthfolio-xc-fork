// Creates one invitation locally. Never writes the plaintext into Git or images.
import { randomBytes, createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import path, { resolve, join } from "node:path";

// Accept a path implementation so both POSIX and Windows rules can be tested.
export function isInsideRepository(repository, destination, paths = path) {
  const relative = paths.relative(repository, destination);
  return (
    relative === "" ||
    (relative !== ".." && !relative.startsWith(".." + paths.sep) && !paths.isAbsolute(relative))
  );
}

if (import.meta.main) {
  const directory = process.argv[2];
  if (!directory)
    throw new Error(
      "Usage: node scripts/create-owner-invitation.mjs /absolute/private-directory-outside-repository",
    );
  const destination = resolve(directory);
  const repository = resolve(import.meta.dirname, "..");
  if (isInsideRepository(repository, destination)) {
    throw new Error("Choose a private directory outside the repository");
  }
  mkdirSync(destination, { recursive: true, mode: 0o700 });
  const code = randomBytes(32).toString("base64url");
  // Exclusive creation prevents accidentally issuing a second invitation.
  writeFileSync(join(destination, "owner-invitation.txt"), code + "\n", {
    mode: 0o600,
    flag: "wx",
  });
  writeFileSync(
    join(destination, "owner-auth.env"),
    "WF_ACCOUNT_AUTH=true\nWF_SIGNUP_INVITE_HASH=" +
      createHash("sha256").update(code).digest("hex") +
      "\n",
    { mode: 0o600, flag: "wx" },
  );
  console.log("Saved the private invitation and server configuration to " + destination);
}
