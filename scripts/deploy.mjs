// Releases the next version: `npm run deploy` on main, once dev is merged into it.
//
// It renames "## Unreleased" in CHANGELOG.md to the version and today's date (the release's notes, and
// what the app shows as What's new), sets the version in Cargo.toml and Cargo.lock, commits that on main
// as "Release <version>", tags it v<version> and pushes both. That release commit on main starts the
// release workflow (.github/workflows/release.yml), which tests on Windows, macOS and Linux, then builds
// and publishes the setup program, the portable zip and the update that installed copies download.
//
// Nothing changes until everything has been checked and you've said yes, and if GitHub turns the push
// down, main is put back as it was. Needs Node.js 18 or later, Git and Cargo; nothing to install.

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { createInterface } from 'node:readline';

const USAGE = `npm run deploy                 the next patch version: 0.2.0 → 0.2.1
npm run deploy -- minor        0.2.0 → 0.3.0
npm run deploy -- major        0.2.0 → 1.0.0
npm run deploy -- 0.4.0        that version
npm run deploy -- --dry-run    shows what it would do, and stops there
npm run deploy -- --yes        doesn't ask first`;

const REMOTE = 'origin';
const BRANCH = 'main';
const DEV = 'dev';
const FILES = ['CHANGELOG.md', 'Cargo.toml', 'Cargo.lock'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

/** Why it stopped, said to the person running it (no stack trace). */
class Stop extends Error {}

/** Runs git and returns what it printed, trimmed. */
function git(...args) {
  return execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 1 << 26 }).trim();
}

/** Runs a command and shows what it prints. */
function run(command, ...args) {
  execFileSync(command, args, { stdio: 'inherit' });
}

function exists(ref) {
  try {
    git('rev-parse', '--verify', '--quiet', ref);
    return true;
  } catch {
    return false;
  }
}

function count(...revisions) {
  return Number(git('rev-list', '--count', ...revisions));
}

function commits(n) {
  return n === 1 ? '1 commit' : `${n} commits`;
}

function options(args) {
  const chosen = { level: 'patch', yes: false, dryRun: process.env.npm_config_dry_run === 'true' };
  let level;
  for (const arg of args) {
    if (arg === '--dry-run' || arg === '-n') chosen.dryRun = true;
    else if (arg === '--yes' || arg === '-y') chosen.yes = true;
    else if (arg === '--help' || arg === '-h') throw new Stop(USAGE);
    else if (!arg.startsWith('-') && !level) level = arg;
    else throw new Stop(`What's "${arg}"?\n\n${USAGE}`);
  }
  chosen.level = level ?? chosen.level;
  return chosen;
}

function lines(text) {
  return { lines: text.split(/\r?\n/), eol: text.includes('\r\n') ? '\r\n' : '\n' };
}

/** The line holding `[package]`'s `key = "…"` in Cargo.toml, and its value. */
function packageLine(rows, key) {
  let inPackage = false;
  for (let at = 0; at < rows.length; at++) {
    const row = rows[at];
    if (/^\s*\[/.test(row)) inPackage = /^\s*\[package\]\s*$/.test(row);
    else if (inPackage) {
      const found = new RegExp(`^${key}\\s*=\\s*"([^"]*)"`).exec(row);
      if (found) return { at, value: found[1] };
    }
  }
  throw new Stop(`Cargo.toml has no ${key} under [package].`);
}

function nextVersion(current, level) {
  const parts = VERSION.exec(current);
  if (!parts) throw new Stop(`Cargo.toml's version, ${current}, isn't three numbers like 1.2.3.`);
  const [major, minor, patch] = parts.slice(1).map(Number);
  const next = { patch: `${major}.${minor}.${patch + 1}`, minor: `${major}.${minor + 1}.0`, major: `${major + 1}.0.0` }[level] ?? level;
  if (!VERSION.test(next)) throw new Stop(`What's "${level}"?\n\n${USAGE}`);
  const [a, b] = [next, current].map((version) => version.split('.').map(Number));
  const later = a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
  if (later <= 0) throw new Stop(`${next} doesn't come after ${current}, the version now.`);
  return next;
}

function today() {
  const now = new Date();
  return `${now.getDate()} ${MONTHS[now.getMonth()]} ${now.getFullYear()}`;
}

/** CHANGELOG.md with "## Unreleased" renamed to the version, and the notes under it. */
function changelog(text, version, date) {
  const { lines: rows, eol } = lines(text);
  const at = rows.findIndex((row) => /^## unreleased\s*$/i.test(row));
  const notes = at < 0 ? [] : rows.slice(at + 1, nextHeading(rows, at));
  if (!notes.some((row) => row.trim())) {
    throw new Stop(
      'CHANGELOG.md has no notes under "## Unreleased". Add what\'s new there first, for the people using the ' +
        "app: they're the release's notes, and what the app shows as What's new.",
    );
  }
  if (rows.some((row) => row.startsWith(`## ${version} `) || row.trim() === `## ${version}`)) {
    throw new Stop(`CHANGELOG.md has a section for ${version} already.`);
  }
  rows[at] = `## ${version} – ${date}`;
  return { text: rows.join(eol), heading: rows[at], notes: notes.join('\n').trim() };
}

function nextHeading(rows, after) {
  const at = rows.findIndex((row, index) => index > after && row.startsWith('## '));
  return at < 0 ? rows.length : at;
}

/** Asks on the terminal; no answer (nothing to read from) is no. */
function ask(question) {
  return new Promise((resolve) => {
    const prompt = createInterface({ input: process.stdin, output: process.stdout });
    prompt.on('close', () => resolve(''));
    prompt.question(question, (answer) => {
      resolve(answer);
      prompt.close();
    });
  });
}

/** The repository's page on GitHub, from the remote's URL (without any credentials in it). */
function webPage(remote) {
  const ssh = /^(?:ssh:\/\/)?git@([^:/]+)[:/](.+?)(?:\.git)?\/?$/.exec(remote);
  if (ssh) return `https://${ssh[1]}/${ssh[2]}`;
  return remote.replace(/^(https?:\/\/)[^@/]*@/, '$1').replace(/(\.git)?\/?$/, '');
}

async function deploy() {
  const chosen = options(process.argv.slice(2));
  process.chdir(git('rev-parse', '--show-toplevel'));

  const branch = git('rev-parse', '--abbrev-ref', 'HEAD');
  if (branch !== BRANCH) throw new Stop(`Run this on ${BRANCH}, once ${DEV} is merged into it (this is ${branch}).`);
  const changed = git('status', '--porcelain', '--untracked-files=no');
  if (changed) throw new Stop(`Commit or stash these first:\n${changed}`);

  console.log(`Looking at ${REMOTE}…`);
  git('fetch', '--quiet', '--tags', REMOTE);
  const upstream = `${REMOTE}/${BRANCH}`;
  if (!exists(`refs/remotes/${upstream}`)) throw new Stop(`There's no ${upstream} to release from.`);
  const ahead = count(`${upstream}..HEAD`);
  const behind = count(`HEAD..${upstream}`);
  if (ahead && behind) {
    throw new Stop(`${BRANCH} and ${upstream} have both moved on. Pull first (git pull), then run this again.`);
  }
  // The release is made from GitHub's main, so a main that's only behind is brought up to date first, as
  // `git pull` would. A dry run changes nothing, so it reads the files from there instead.
  let base = 'HEAD';
  if (behind && chosen.dryRun) base = upstream;
  else if (behind) {
    git('merge', '--quiet', '--ff-only', upstream);
    console.log(`${BRANCH} was ${commits(behind)} behind ${upstream}: brought it up to date.`);
  }
  const read = (file) => (base === 'HEAD' ? readFileSync(file, 'utf8') : git('show', `${base}:${file}`));

  const manifest = lines(read('Cargo.toml'));
  const name = packageLine(manifest.lines, 'name').value;
  const version = packageLine(manifest.lines, 'version');
  const next = nextVersion(version.value, chosen.level);
  const tag = `v${next}`;
  if (exists(`refs/tags/${tag}`) || git('ls-remote', '--tags', REMOTE, `refs/tags/${tag}`)) {
    throw new Stop(`There's a tag ${tag} already.`);
  }
  const notes = changelog(read('CHANGELOG.md'), next, today());
  const devs = [`refs/heads/${DEV}`, `refs/remotes/${REMOTE}/${DEV}`].filter(exists);
  const unmerged = devs.length ? count(...devs, `^${base}`) : 0;

  console.log(`\nAsset Browser ${version.value} → ${next}, from ${BRANCH}.\n`);
  console.log(`The notes under "## Unreleased" in CHANGELOG.md, which becomes "${notes.heading}":\n`);
  console.log(`${notes.notes.replace(/^/gm, '  ')}\n`);
  console.log(
    `It commits that, with the version in Cargo.toml and Cargo.lock, as "Release ${next}" on ${BRANCH}, tags it\n` +
      `${tag} and pushes both to ${REMOTE}, which starts the release.`,
  );
  if (ahead) console.log(`${BRANCH} has ${commits(ahead)} that ${upstream} doesn't, pushed with it.`);
  if (behind && chosen.dryRun) {
    console.log(`${BRANCH} is ${commits(behind)} behind ${upstream}: it'd be brought up to date first.`);
  }
  if (unmerged) console.log(`\n${DEV} has ${commits(unmerged)} that ${BRANCH} doesn't, which won't be in this release.`);
  if (chosen.dryRun) {
    console.log('\nThat was a dry run: nothing changed.');
    return;
  }
  if (!chosen.yes && !/^y(es)?$/i.test((await ask(`\nRelease ${next}? [y/N] `)).trim())) {
    console.log('Nothing released.');
    return;
  }

  const before = git('rev-parse', 'HEAD');
  try {
    writeFileSync('CHANGELOG.md', notes.text);
    manifest.lines[version.at] = manifest.lines[version.at].replace(/"[^"]*"/, `"${next}"`);
    writeFileSync('Cargo.toml', manifest.lines.join(manifest.eol));
    // Only this package's own entry changes; the release builds with --locked, which checks it.
    execFileSync('cargo', ['update', '--workspace', '--offline', '--quiet'], { stdio: 'inherit' });
    execFileSync('cargo', ['metadata', '--locked', '--offline', '--format-version', '1'], { stdio: ['ignore', 'ignore', 'inherit'] });
    const locked = new RegExp(`\\[\\[package\\]\\]\\r?\\nname = "${name}"\\r?\\nversion = "([^"]+)"`).exec(readFileSync('Cargo.lock', 'utf8'));
    if (locked?.[1] !== next) throw new Stop(`Cargo.lock has ${name} at ${locked?.[1] ?? 'no version'}, not ${next}.`);
    git('add', '--', ...FILES);
    const body =
      `- CHANGELOG.md's Unreleased notes are now ${next}'s: the release's notes,\n` +
      `  and what the app shows as What's new after updating to it\n` +
      `- Cargo.toml and Cargo.lock are at ${next}`;
    git('commit', '--quiet', '-m', `Release ${next}`, '-m', body);
  } catch (error) {
    git('restore', '--source=HEAD', '--staged', '--worktree', '--', ...FILES);
    throw error;
  }
  git('tag', '-a', tag, '-m', `Asset Browser ${next}`);

  // Both or neither: the workflow publishes main's release commit under its tag, and stops without it.
  try {
    run('git', 'push', '--atomic', REMOTE, `refs/heads/${BRANCH}`, `refs/tags/${tag}`);
  } catch {
    git('tag', '-d', tag);
    git('reset', '--quiet', '--keep', before);
    throw new Stop(
      `The push didn't go through (see above), so nothing was released, and ${BRANCH} is back as it was.\n` +
        `If GitHub said changes must be made through a pull request, or that status checks are required, the\n` +
        `ruleset for ${BRANCH} has to let you push to it. On GitHub, Settings → Rules → Rulesets → ${BRANCH}: turn\n` +
        `off "Require status checks to pass", and either turn off "Require a pull request before merging" too, or\n` +
        `add Repository admin to the bypass list as "Always allow". Then run this again.`,
    );
  }

  const page = webPage(git('remote', 'get-url', REMOTE));
  console.log(`\nPushed ${tag}. GitHub tests it on Windows, macOS and Linux, then builds and publishes it, which`);
  console.log(`takes 10 to 30 minutes: ${page}/actions/workflows/release.yml`);
  // dev has to have the renamed notes before more go under a new "## Unreleased", or the next merge
  // conflicts. If it has nothing main doesn't, it's simply moved up to main.
  if (exists(`refs/heads/${DEV}`)) {
    try {
      git('fetch', '--quiet', '.', `refs/heads/${BRANCH}:refs/heads/${DEV}`);
      console.log(`${DEV} is moved up to ${BRANCH} too, so it has the release; push it when you like.`);
    } catch {
      console.log(`Merge ${BRANCH} into ${DEV} before carrying on there (git switch ${DEV}, git merge ${BRANCH}).`);
    }
  }
}

deploy().catch((error) => {
  if (error instanceof Stop) console.error(`\n${error.message}`);
  else if (error.code === 'ENOENT') console.error(`\n${error.path} wasn't found: is it installed?`);
  else console.error(`\n${error.message}`);
  process.exitCode = 1;
});
