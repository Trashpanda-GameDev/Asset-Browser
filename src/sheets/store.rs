//! The user's own choices for each picture, kept in the user data file.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::{Grid, Split};
use crate::{db, diag};

/// What the user chose for one picture: to see it as an animation, their grid, and the speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Saved {
    pub animate: bool,
    pub grid: Option<Grid>,
    /// The speed the user picked; `None` plays at the default speed, whatever that is now.
    pub fps: Option<f32>,
}

/// Per-picture sheet choices, in the user data file beside the favourites and collections.
pub struct SheetStore {
    conn: Option<Connection>,
}

impl SheetStore {
    pub fn open(path: &Path) -> Self {
        match db::open_user_data(path) {
            Ok(conn) => Self { conn: Some(conn) },
            Err(err) => {
                diag::report("Sprite sheet settings unavailable", err);
                Self { conn: None }
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<Saved> {
        let conn = self.conn.as_ref()?;
        conn.query_row(
            "SELECT animate, columns, rows, margin, spacing, split, fps FROM sheets WHERE key = ?1",
            [key],
            |row| {
                let columns: Option<u32> = row.get(1)?;
                let grid = match columns {
                    Some(columns) => Some(Grid {
                        columns,
                        rows: row.get::<_, Option<u32>>(2)?.unwrap_or(1),
                        margin: row.get::<_, Option<u32>>(3)?.unwrap_or(0),
                        spacing: row.get::<_, Option<u32>>(4)?.unwrap_or(0),
                        split: Split::from_db(row.get::<_, Option<i64>>(5)?.unwrap_or(0)),
                    }),
                    None => None,
                };
                let fps: f64 = row.get(6)?;
                Ok(Saved { animate: row.get::<_, i64>(0)? != 0, grid, fps: (fps > 0.0).then_some(fps as f32) })
            },
        )
        .optional()
        .map_err(|err| diag::report("Reading sprite sheet settings", err))
        .ok()
        .flatten()
    }

    /// Forgets every picture's choices (Settings → Storage).
    pub fn clear(&self) {
        if let Some(conn) = &self.conn
            && let Err(err) = conn.execute("DELETE FROM sheets", [])
        {
            diag::report("Clearing sprite sheet choices", err);
        }
    }

    pub fn set(&self, key: &str, saved: &Saved) {
        let Some(conn) = &self.conn else { return };
        let grid = saved.grid;
        let result = conn.execute(
            "INSERT OR REPLACE INTO sheets(key, animate, columns, rows, margin, spacing, split, fps)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                key,
                saved.animate,
                grid.map(|g| g.columns),
                grid.map(|g| g.rows),
                grid.map(|g| g.margin),
                grid.map(|g| g.spacing),
                grid.map(|g| g.split.to_db()),
                saved.fps.unwrap_or(0.0),
            ],
        );
        if let Err(err) = result {
            diag::report("Saving sprite sheet settings", err);
        }
    }
}
