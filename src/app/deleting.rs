//! Deleting files and folders to the Recycle Bin: the question, the job on its own thread, and
//! the library catching up afterwards.

use std::collections::HashSet;
use std::path::PathBuf;

use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;

use crate::deletion::{self, Deleted};
use crate::groups::UNCATEGORIZED;
use crate::library;
use crate::scan::ScannedFile;
use crate::ui::format::{grouped, plural};
use crate::ui::organize::{GroupCommand, Targets};
use crate::ui::question::{self, Answer, Answered, Question};
use crate::ui::theme;
use crate::{diag, platform};

use super::BrowserApp;

/// Files or a folder to delete, waiting for "Delete?" to be answered.
pub(super) struct DeleteRequest {
    paths: Vec<PathBuf>,
    /// For the messages: "3 files", "\u{201c}hero.png\u{201d}", "the folder \u{201c}ui\u{201d}".
    what: String,
    /// The dialog's question, and what it lists.
    title: String,
    details: String,
    /// On a drive with no Recycle Bin, so it can't be undone.
    permanent: bool,
    /// The group being browsed: the files could just be taken out of it instead.
    group: Option<i64>,
    after: AfterDelete,
}

/// Where to be once the files are gone, as in a file manager.
#[derive(Clone, Default)]
struct AfterDelete {
    /// The asset after them in the view, to select (by cache key).
    next: Option<i64>,
    /// A deleted folder and its parent (`DirNode::key`s): the parent is browsed instead if the
    /// folder, or one inside it, was.
    folder: Option<String>,
    parent: Option<String>,
}

/// A deletion under way.
pub(super) struct Deleting {
    rx: Receiver<Deleted>,
    what: String,
    permanent: bool,
    after: AfterDelete,
}

/// Files or a folder waiting for "Delete?" to be confirmed, and a deletion under way.
#[derive(Default)]
pub(super) struct Deletion {
    pub(super) confirm_files: Option<DeleteRequest>,
    pub(super) deleting: Option<Deleting>,
}

impl BrowserApp {
    /// "Delete these?" for the selection: the Delete key, or the asset menu.
    pub(super) fn ask_to_delete_selection(&mut self) {
        if self.deletion.deleting.is_some() {
            self.notify("Still deleting the last ones…".to_owned());
            return;
        }
        let items = self.browsing.selected_in_order();
        let Some(library) = &self.library else { return };
        if items.is_empty() {
            return;
        }
        let entry = |index: u32| library.entry(index);
        let paths: Vec<PathBuf> = items.iter().map(|&index| library.path_of(entry(index))).collect();
        let names: Vec<&str> = items.iter().take(3).map(|&index| entry(index).name()).collect();
        // One file is named in the question, so this says where it is; several are listed.
        let details = match items.as_slice() {
            &[index] => {
                let root = &library.root(entry(index).root).name;
                let folder = entry(index).folder();
                if folder.is_empty() {
                    format!("In {root}")
                } else {
                    format!("In {root} › {}", folder.replace('/', " › "))
                }
            }
            _ if items.len() > names.len() => {
                format!("{} and {} more", names.join(", "), grouped(items.len() - names.len()))
            }
            _ => names.join(", "),
        };
        // The asset after them takes the selection afterwards, or else the one before.
        let chosen: HashSet<u32> = items.iter().copied().collect();
        let focus = self.browsing.mode_view().selection.focus;
        let at = focus.and_then(|focus| self.browsing.view.iter().position(|&index| index == focus)).unwrap_or(0);
        let next = self.browsing.view[at..]
            .iter()
            .chain(self.browsing.view[..at].iter().rev())
            .find(|index| !chosen.contains(index))
            .map(|&index| entry(index).key);
        let what = match names.as_slice() {
            [name] if items.len() == 1 => format!("\u{201c}{name}\u{201d}"),
            _ => plural(items.len(), "file"),
        };
        self.deletion.confirm_files = Some(DeleteRequest {
            title: format!("Delete {what}?"),
            details,
            permanent: !paths.iter().all(|path| platform::has_trash(path)),
            group: self.browsing.group.filter(|&id| id != UNCATEGORIZED),
            after: AfterDelete { next, ..AfterDelete::default() },
            paths,
            what,
        });
    }

    /// "Delete this folder?" from its menu in the tree. The library's own folders are only ever
    /// taken out of the library (`remove_root`), never deleted from here.
    pub(super) fn ask_to_delete_folder(&mut self, dir: u32) {
        if self.deletion.deleting.is_some() {
            self.notify("Still deleting the last ones…".to_owned());
            return;
        }
        let Some(library) = &self.library else { return };
        let Some(node) = library.dirs().get(dir as usize).filter(|node| node.root.is_some() && !node.lower.is_empty())
        else {
            return;
        };
        let Some(path) = library.dir_path(dir) else { return };
        let assets: u32 = node.totals.iter().sum();
        let parent =
            node.parent.and_then(|parent| library.dirs().get(parent as usize)).map(|parent| parent.key.clone());
        let what = format!("the folder \u{201c}{}\u{201d}", node.name);
        self.deletion.confirm_files = Some(DeleteRequest {
            title: format!("Delete {what}?"),
            details: format!(
                "Everything in it goes with it: {} and any other files.",
                plural(assets as usize, "asset")
            ),
            permanent: !platform::has_trash(&path),
            group: None,
            after: AfterDelete { next: None, folder: Some(node.key.clone()), parent },
            paths: vec![path],
            what,
        });
    }

    /// "Delete?", saying where things go. Enter deletes when it can be undone; Esc cancels.
    pub(super) fn delete_files_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.deletion.confirm_files else { return };
        let bin = platform::trash_name();
        let note = if request.permanent {
            RichText::new(format!("{}  This drive has no {bin}, so this can't be undone.", icon::WARNING))
                .color(theme::ERROR)
        } else {
            RichText::new(format!("Deleted files and folders go to the {bin}, so they can be put back."))
                .color(theme::TEXT_FAINT)
        };
        // Browsing a group, the files can be taken out of it instead.
        let take_out = request
            .group
            .and_then(|id| self.organizing.groups.get(id))
            .map(|group| (group.id, format!("Only take out of {}", group.name)));
        let asked = Question {
            id: "delete-files",
            width: 420.0,
            title: &request.title,
            details: &request.details,
            note: Some(note.size(12.5)),
            confirm: Answer {
                glyph: icon::TRASH,
                label: if request.permanent { "Delete for good" } else { "Delete" },
                tooltip: None,
            },
            other: take_out.as_ref().map(|(_, label)| Answer {
                glyph: icon::MINUS_CIRCLE,
                label,
                tooltip: Some("The files stay where they are"),
            }),
            enter_confirms: !request.permanent,
        };
        let Some(answered) = question::ask(ctx, &asked) else { return };
        let Some(request) = self.deletion.confirm_files.take() else { return };
        match (answered, take_out) {
            (Answered::Confirm, _) => self.start_deleting(request),
            (Answered::Other, Some((id, _))) => {
                self.run_group_command(GroupCommand::Remove(id, Targets::Paths(request.paths)));
            }
            (Answered::Other | Answered::Cancel, _) => {}
        }
    }

    fn start_deleting(&mut self, request: DeleteRequest) {
        // A sound that's being deleted stops first.
        if let (Some(library), Some((index, _))) = (&self.library, self.playback.playing)
            && let Some(entry) = library.entries().get(index as usize)
        {
            let playing = library.path_of(entry);
            if request.paths.iter().any(|path| library::is_within(&playing, path)) {
                self.playback.reset();
            }
        }
        let ctx = self.ctx.clone();
        let rx = deletion::start(request.paths, move || ctx.request_repaint());
        self.deletion.deleting =
            Some(Deleting { rx, what: request.what, permanent: request.permanent, after: request.after });
    }

    /// Takes in a finished deletion: drops what went from the library, moves to what's next, and
    /// says how it went.
    pub(super) fn poll_deleting(&mut self) {
        let Some(job) = self.deletion.deleting.take() else { return };
        let deleted = match job.rx.try_recv() {
            Ok(deleted) => deleted,
            Err(crossbeam_channel::TryRecvError::Empty) => {
                self.deletion.deleting = Some(job);
                return;
            }
            // The thread couldn't start (that's reported) or died.
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.notify(format!("Couldn't delete {}", job.what));
                return;
            }
        };
        if !deleted.gone.is_empty() {
            // Browsing the deleted folder, or one inside it: its parent is shown instead.
            let browsing_it = job.after.folder.as_ref().is_some_and(|folder| {
                let shown =
                    self.library.as_ref().and_then(|library| library.dirs().get(self.browsing.selected_dir as usize));
                self.browsing.group.is_none()
                    && shown.is_some_and(|dir| dir.key == *folder || dir.key.starts_with(&format!("{folder}/")))
            });
            self.forget_paths(&deleted.gone);
            if browsing_it
                && let Some(dir) = job.after.parent.as_ref().and_then(|parent| self.library.as_ref()?.find_dir(parent))
            {
                self.select_dir(dir);
            }
            // The asset after the deleted ones takes the selection, as in a file manager. The view
            // is rebuilt first: it held indices into the old library.
            self.refresh_membership();
            if self.browsing.view_dirty {
                self.rebuild_view();
            }
            let next = job.after.next.and_then(|key| {
                let library = self.library.as_ref()?;
                library.entries().iter().position(|entry| entry.key == key).map(|index| index as u32)
            });
            if self.browsing.mode_view().selection.is_empty()
                && let Some(index) = next.filter(|index| self.browsing.view.contains(index))
            {
                self.browsing.mode_view_mut().selection.only(index);
                self.browsing.scroll_to_focus = true;
                self.focused(index);
            }
        }
        match &deleted.error {
            None if job.permanent => self.notify(format!("Deleted {}", job.what)),
            None => self.notify(format!("Moved {} to the {}", job.what, platform::trash_name())),
            Some(error) => {
                let mut kept: Vec<String> = deleted.kept.iter().take(3).map(|path| platform::native(path)).collect();
                if deleted.kept.len() > kept.len() {
                    kept.push(format!("and {} more", grouped(deleted.kept.len() - kept.len())));
                }
                diag::report(&format!("Couldn't delete {}", kept.join(", ")), error);
                self.notify(if deleted.gone.is_empty() {
                    format!("Couldn't delete {}: see the status bar", job.what)
                } else {
                    format!("Deleted {}, but not {}", plural(deleted.gone.len(), "file"), grouped(deleted.kept.len()))
                });
            }
        }
    }

    /// Drops deleted files and folders from the library at once; a rescan of their library
    /// folders follows, to make sure and to save the index.
    fn forget_paths(&mut self, gone: &[PathBuf]) {
        let Some(library) = &self.library else { return };
        let mut by_root: Vec<(u16, Vec<String>)> = Vec::new();
        for path in gone {
            let Some((root, rel)) = library.place_of(path) else { continue };
            match by_root.iter_mut().find(|(other, _)| *other == root) {
                Some((_, rels)) => rels.push(rel),
                None => by_root.push((root, vec![rel])),
            }
        }
        let fresh: Vec<(PathBuf, Vec<ScannedFile>)> = by_root
            .iter()
            .map(|(root, rels)| (library.root(*root).path.clone(), library.files_without(*root, rels)))
            .collect();
        if fresh.is_empty() {
            return;
        }
        let rescan: Vec<PathBuf> = fresh.iter().map(|(path, _)| path.clone()).collect();
        self.rebuild_library(fresh);
        self.scan(rescan);
    }
}
