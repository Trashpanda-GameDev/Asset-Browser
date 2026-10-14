# Releasing

How a new version gets to the people using the app. [Back to the README](../README.md)

## Making a release

What's new goes in [`CHANGELOG.md`](../CHANGELOG.md) as it's made, under **Unreleased**, written for the people using
the app. To release it, merge `dev` into `main`, then on `main` (with [Node.js](https://nodejs.org) 18 or later):

```
git switch main
git pull
npm run deploy
```

That makes the next patch version, 0.2.0 → 0.2.1 (`npm run deploy -- minor` for 0.3.0, `-- major` for 1.0.0, or
`-- 0.4.0` for that one). It shows the version and the notes and asks first (`npm run deploy -- --dry-run` only
shows them). Then `scripts/deploy.mjs`:

1. renames `## Unreleased` in `CHANGELOG.md` to the version and today's date (`## 0.2.1 – 13 Oct 2026`): the
   release's notes, and the app's "What's new";
2. sets the version in `Cargo.toml` and `Cargo.lock` (the release builds with `--locked`, so both need it);
3. commits that as "Release 0.2.1" on `main`, tags it `v0.2.1` and pushes both, which starts the release;
4. moves `dev` up to `main` if it has nothing `main` doesn't, so the renamed notes are there too (push it when you
   like). Otherwise merge `main` into `dev` before carrying on, or the next merge conflicts over the renamed
   heading.

It stops before changing anything if it isn't on `main`, there are uncommitted changes, `main` and GitHub's have
both moved on, the tag exists or there are no notes; a `main` that's only behind is brought up to date first, as
`git pull` would. If GitHub turns the push down, `main` is put back as it was.

Don't make the release on GitHub's website (**Draft a new release**): the workflow makes it, and fails if the tag has
one already.

## The ruleset for `main`

The push goes straight to `main`, so the ruleset for `main` (**Settings**, **Rules**, **Rulesets**) has to let it
through. Turn off **Require status checks to pass** either way, since those checks don't run any more, and then
either:

- turn off **Require a pull request before merging** too, keeping **Restrict deletions** and **Block force
  pushes**; or
- add **Repository admin** to the bypass list as **Always allow** (*For pull requests only* doesn't cover a push).
  GitHub then answers the push with "Bypassed rule violations for refs/heads/main", which is expected. A bypass
  covers every rule in its ruleset, deletions and force pushes too, so to keep those blocked for you as well, put
  them in a second ruleset for `main` with nothing on its bypass list.

## The workflow

`.github/workflows/release.yml` is the only workflow, and it runs only for a release: the "Release 0.2.1" commit that
`npm run deploy` pushes to `main` with its tag. Other pushes and pull requests run nothing (a merge that changes
`Cargo.toml` shows as a skipped run). It tests on Windows, macOS and Linux (`cargo fmt`, `clippy` and `test`), then
builds the app on Windows, packs it with [Velopack](https://velopack.io) into the setup program, the portable zip and
the update packages (with a smaller delta from the last release), and publishes them as a GitHub release, under the
tag, with the version's notes from `CHANGELOG.md`. A failing test, or a commit without its tag, stops it before
anything is published. To test, build and pack without publishing, run it by hand on any branch (**Actions**,
**Release**, **Run workflow**): the files are kept with the run.

The builds are cached on `main`, which is why a release starts from `main`'s commit rather than from the tag: a run
can only use caches made on its own branch or tag, or on `main`. Only releases save caches, and only when the
dependencies or Rust have changed, since the app's version isn't part of a cache's name. After each release, the
workflow keeps the newest two caches of each job, that release's and the one before, and deletes the rest, so they
take about 6 GB at most of the repository's 10 GB. With a cache, a release should take about 10 to 15 minutes;
without one, 20 to 30. GitHub deletes caches that haven't been used for 7 days, so a release more than a week after
the last one starts without.

The app id the workflow packs with, `TrashpandaAssetBrowser`, must never change, because installed copies find their
updates by it. Velopack's packer, `vpk`, is installed at the version of the `velopack` crate in `Cargo.lock`; the two
have to match.

## Trying an update before releasing it

With the .NET 8 SDK or later, pack a build twice, as two versions, into one folder:

```
dotnet tool install -g vpk --version 1.2.161
cargo build --release
mkdir target\publish
copy target\release\asset_browser.exe target\publish\
vpk pack -u AssetBrowserTest -v 0.1.0 -p target\publish -e asset_browser.exe -r win-x64 -o target\velopack
```

Unzip `target\velopack\AssetBrowserTest-win-Portable.zip` somewhere and run the `vpk pack` line again with `-v 0.1.1`.
Then start the unzipped copy with `ASSET_BROWSER_UPDATE_FEED` set to that folder, from PowerShell in the repository:

```
$env:ASSET_BROWSER_UPDATE_FEED = "$PWD\target\velopack"
& "<where you unzipped it>\AssetBrowserTest.exe"
```

It looks there instead of on GitHub, finds 0.1.1 and updates to it. The test id keeps it apart from a real install. A
copy built with `cargo build` doesn't update itself.
