//! Updates, through Velopack. A copy installed with the setup program, or unzipped from the
//! portable zip, checks GitHub for a newer release when it starts, every few hours after that and
//! when asked. It downloads one in the background and installs it when the app restarts or
//! closes. A copy run straight from a build isn't installed, so it never checks.
//!
//! `.github/workflows/release.yml` packs and publishes the releases, and `main` hands the
//! installer's and updater's visits to Velopack before anything else runs.

use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender};
use velopack::sources::AutoSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager};

use crate::diag;
use crate::work::{self, Wake};

/// Where releases are published.
pub const RELEASES: &str = "https://github.com/Trashpanda-GameDev/Asset-Browser";
/// A folder or web address holding `vpk pack` output, checked instead of `RELEASES`: for trying an
/// update out before it's released.
const FEED_VARIABLE: &str = "ASSET_BROWSER_UPDATE_FEED";
/// How often a copy that's left open checks again. A check uses one or two of the 60 requests an
/// hour GitHub allows an address that doesn't sign in.
const CHECK_AGAIN: Duration = Duration::from_secs(6 * 60 * 60);

/// What the updater is doing.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Run from a build rather than installed: it doesn't update itself.
    NotInstalled,
    Checking,
    /// Nothing newer was found, or it hasn't checked yet.
    UpToDate,
    Downloading {
        version: String,
        percent: u8,
    },
    /// Downloaded: it's installed when the app restarts or closes.
    Ready {
        version: String,
    },
    /// Checking, downloading or installing failed, most often for being offline. `action` is what
    /// couldn't be done: "check for updates".
    Failed {
        action: &'static str,
        error: String,
    },
}

/// News from a check, which runs on its own thread.
enum Event {
    UpToDate,
    Downloading(String),
    Progress(u8),
    Downloaded(Box<UpdateInfo>),
    Failed { action: &'static str, error: String },
}

pub struct Updates {
    /// `None` when not installed.
    manager: Option<UpdateManager>,
    status: Status,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    repaint: Wake,
    /// When the last check started, to check again.
    started: Option<Instant>,
    /// When a check last found nothing newer.
    checked: Option<SystemTime>,
    asked: Asked,
    download: Download,
}

/// A check someone asked for, whose answer is shown for a moment.
enum Asked {
    No,
    /// Under way.
    Waiting,
    Answered(Instant),
}

/// The update that was downloaded.
enum Download {
    None,
    Ready(Box<UpdateInfo>),
    /// Handed to Velopack's updater, which installs it once the app has closed.
    Installing,
}

impl Updates {
    /// Starts the first check, when installed. `repaint` wakes the window when there's news.
    pub fn start(repaint: Wake) -> Self {
        let feed = std::env::var(FEED_VARIABLE).ok().filter(|feed| !feed.trim().is_empty());
        let source = AutoSource::new(feed.as_deref().unwrap_or(RELEASES));
        // Fails when the app isn't installed: there's no `Update.exe` beside its folder.
        let manager = UpdateManager::new(source, None, None).ok();
        let status = if manager.is_some() { Status::UpToDate } else { Status::NotInstalled };
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut updates = Self {
            manager,
            status,
            tx,
            rx,
            repaint,
            started: None,
            checked: None,
            asked: Asked::No,
            download: Download::None,
        };
        updates.check(false);
        updates
    }

    /// This copy's version: the release it was installed from, or the build's.
    pub fn version(&self) -> String {
        match &self.manager {
            Some(manager) => manager.get_current_version_as_string(),
            None => env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// Unzipped from the portable zip rather than installed with the setup program.
    pub fn is_portable(&self) -> bool {
        self.manager.as_ref().is_some_and(UpdateManager::get_is_portable)
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// The check under way was asked for.
    pub fn asked(&self) -> bool {
        matches!(self.asked, Asked::Waiting)
    }

    /// When a check that was asked for ended, to show its answer for a moment.
    pub fn answered(&self) -> Option<Instant> {
        match self.asked {
            Asked::Answered(at) => Some(at),
            Asked::No | Asked::Waiting => None,
        }
    }

    /// When a check last found nothing newer.
    pub fn checked(&self) -> Option<SystemTime> {
        self.checked
    }

    /// Checks now, and shows the answer.
    pub fn check_now(&mut self) {
        self.check(true);
    }

    fn check(&mut self, asked: bool) {
        let Some(manager) = &self.manager else { return };
        if !matches!(self.status, Status::UpToDate | Status::Failed { .. }) {
            return;
        }
        let (manager, tx, repaint) = (manager.clone(), self.tx.clone(), self.repaint.clone());
        if work::spawn("updates", move || check_and_download(&manager, &tx, &*repaint)).is_some() {
            self.status = Status::Checking;
            self.started = Some(Instant::now());
            self.asked = if asked { Asked::Waiting } else { Asked::No };
        } else {
            self.status = Status::Failed { action: "check for updates", error: "the check couldn't start".to_owned() };
        }
    }

    /// Takes in what the check found, and checks again when it's time. Called every frame.
    pub fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::UpToDate => {
                    self.status = Status::UpToDate;
                    self.checked = Some(SystemTime::now());
                    self.answer();
                }
                Event::Downloading(version) => self.status = Status::Downloading { version, percent: 0 },
                Event::Progress(percent) => {
                    if let Status::Downloading { percent: shown, .. } = &mut self.status {
                        *shown = percent;
                    }
                }
                Event::Downloaded(update) => {
                    self.status = Status::Ready { version: update.TargetFullRelease.Version.clone() };
                    self.download = Download::Ready(update);
                    self.answer();
                }
                Event::Failed { action, error } => {
                    self.status = Status::Failed { action, error };
                    self.answer();
                }
            }
        }
        if self.started.is_some_and(|started| started.elapsed() >= CHECK_AGAIN) {
            self.check(false);
        }
    }

    fn answer(&mut self) {
        if let Asked::Waiting = self.asked {
            self.asked = Asked::Answered(Instant::now());
        }
    }

    /// Hands the download to Velopack's updater, which installs it and starts the app again once
    /// it has closed. True when the app should now close.
    pub fn restart(&mut self) -> bool {
        self.install(true)
    }

    /// Installs a download that wasn't restarted for, quietly, once the app has closed.
    pub fn on_exit(&mut self) {
        self.install(false);
    }

    fn install(&mut self, restart: bool) -> bool {
        let (Some(manager), Download::Ready(update)) = (&self.manager, &self.download) else { return false };
        // Restarting shows the updater's progress; closing doesn't pop anything up afterwards.
        match manager.wait_exit_then_apply_updates(&**update, !restart, restart, Vec::<&str>::new()) {
            Ok(()) => {
                self.download = Download::Installing;
                true
            }
            Err(err) => {
                diag::report("Couldn't install the update", &err);
                self.status = Status::Failed { action: "install the update", error: err.to_string() };
                false
            }
        }
    }
}

/// Checks for a newer release and downloads it, sending news as it goes.
fn check_and_download(manager: &UpdateManager, tx: &Sender<Event>, repaint: &(dyn Fn() + Send + Sync)) {
    let send = |event| {
        // Fails only when the window has closed.
        let _ = tx.send(event);
        repaint();
    };
    let update = match manager.check_for_updates() {
        Ok(UpdateCheck::UpdateAvailable(update)) => update,
        Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty) => return send(Event::UpToDate),
        Err(err) => return send(failed("check for updates", &err)),
    };
    send(Event::Downloading(update.TargetFullRelease.Version.clone()));
    // Velopack reports every 5%, until the download ends and drops its end of the channel.
    let (progress_tx, progress_rx) = std::sync::mpsc::channel::<i16>();
    let downloaded = std::thread::scope(|scope| {
        let send = &send;
        scope.spawn(move || {
            for percent in progress_rx {
                send(Event::Progress(percent.clamp(0, 100) as u8));
            }
        });
        manager.download_updates(&update, Some(progress_tx))
    });
    send(match downloaded {
        Ok(()) => Event::Downloaded(update),
        Err(err) => failed("download the update", &err),
    });
}

/// Logged, but shown only when the check was asked for: being offline isn't worth a warning.
fn failed(action: &'static str, err: &velopack::Error) -> Event {
    diag::note(&format!("Couldn't {action}"), err);
    Event::Failed { action, error: err.to_string() }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn a_build_never_checks() {
        // Tests run from a build, which isn't installed.
        let mut updates = Updates::start(Arc::new(|| {}));
        assert_eq!(updates.status(), &Status::NotInstalled);
        assert_eq!(updates.version(), env!("CARGO_PKG_VERSION"));
        assert!(!updates.is_portable());
        updates.check_now();
        updates.poll();
        assert_eq!(updates.status(), &Status::NotInstalled);
        assert!(!updates.asked());
    }

    #[test]
    fn news_moves_the_status_on() {
        let mut updates = Updates::start(Arc::new(|| {}));
        updates.tx.send(Event::Downloading("9.9.9".into())).unwrap();
        updates.tx.send(Event::Progress(45)).unwrap();
        updates.poll();
        assert_eq!(updates.status(), &Status::Downloading { version: "9.9.9".into(), percent: 45 });

        let mut update = UpdateInfo::default();
        update.TargetFullRelease.Version = "9.9.9".into();
        updates.tx.send(Event::Downloaded(Box::new(update))).unwrap();
        updates.poll();
        assert_eq!(updates.status(), &Status::Ready { version: "9.9.9".into() });
        // Without Velopack's updater there's nothing to restart into.
        assert!(!updates.restart());
    }

    #[test]
    fn answers_show_only_when_asked() {
        let mut updates = Updates::start(Arc::new(|| {}));
        let offline = Status::Failed { action: "check for updates", error: "offline".into() };
        updates.tx.send(Event::Failed { action: "check for updates", error: "offline".into() }).unwrap();
        updates.poll();
        assert_eq!(updates.status(), &offline);
        assert!(updates.answered().is_none());

        updates.asked = Asked::Waiting;
        updates.tx.send(Event::UpToDate).unwrap();
        updates.poll();
        assert_eq!(updates.status(), &Status::UpToDate);
        assert!(updates.checked().is_some());
        assert!(updates.answered().is_some());
        assert!(!updates.asked());
    }
}
