use anyhow::Result;
use rusqlite::Connection;

pub fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS media_items (
            id          INTEGER PRIMARY KEY,
            path        TEXT NOT NULL UNIQUE,
            mtime       INTEGER NOT NULL,
            media_date  INTEGER NOT NULL,
            year        INTEGER NOT NULL,
            month       INTEGER NOT NULL,
            media_type  TEXT NOT NULL,
            width       INTEGER,
            height      INTEGER,
            thumb_path  TEXT,
            thumb_ready INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_date  ON media_items(year DESC, month DESC, media_date DESC);
        CREATE INDEX IF NOT EXISTS idx_thumb ON media_items(thumb_ready);
        CREATE INDEX IF NOT EXISTS idx_path  ON media_items(path);",
    )?;
    Ok(())
}
