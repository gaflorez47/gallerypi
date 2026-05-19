use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct MediaItem {
    pub id: i64,
    pub path: String,
    pub mtime: i64,
    pub media_date: i64,
    pub year: i32,
    pub month: i32,
    pub media_type: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub thumb_path: Option<String>,
    pub thumb_ready: bool,
}

#[derive(Debug, Clone)]
pub struct MonthGroup {
    pub year: i32,
    pub month: i32,
    pub count: i64,
}

pub fn upsert_item(conn: &Connection, item: &MediaItem) -> Result<()> {
    conn.execute(
        "INSERT INTO media_items
            (path, mtime, media_date, year, month, media_type, width, height, thumb_path, thumb_ready)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(path) DO UPDATE SET
            mtime=excluded.mtime,
            media_date=excluded.media_date,
            year=excluded.year,
            month=excluded.month,
            media_type=excluded.media_type,
            width=excluded.width,
            height=excluded.height,
            thumb_path=excluded.thumb_path,
            thumb_ready=excluded.thumb_ready",
        params![
            item.path,
            item.mtime,
            item.media_date,
            item.year,
            item.month,
            item.media_type,
            item.width,
            item.height,
            item.thumb_path,
            item.thumb_ready as i32,
        ],
    )?;
    Ok(())
}

pub fn get_existing_mtime(conn: &Connection, path: &str) -> Result<Option<i64>> {
    let result = conn.query_row(
        "SELECT mtime FROM media_items WHERE path = ?1",
        params![path],
        |row| row.get(0),
    );
    match result {
        Ok(mtime) => Ok(Some(mtime)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn mark_thumb_ready(conn: &Connection, id: i64, thumb_path: &str) -> Result<()> {
    conn.execute(
        "UPDATE media_items SET thumb_path = ?1, thumb_ready = 1 WHERE id = ?2",
        params![thumb_path, id],
    )?;
    Ok(())
}

pub fn get_items_by_month(
    conn: &Connection,
    year: i32,
    month: i32,
) -> Result<Vec<MediaItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, mtime, media_date, year, month, media_type, width, height, thumb_path, thumb_ready
         FROM media_items
         WHERE year = ?1 AND month = ?2
         ORDER BY media_date ASC",
    )?;
    let items = stmt.query_map(params![year, month], row_to_item)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
}

pub fn get_all_items_ordered(conn: &Connection) -> Result<Vec<MediaItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, mtime, media_date, year, month, media_type, width, height, thumb_path, thumb_ready
         FROM media_items
         ORDER BY year DESC, month DESC, media_date ASC",
    )?;
    let items = stmt.query_map([], row_to_item)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
}

pub fn get_month_groups(conn: &Connection) -> Result<Vec<MonthGroup>> {
    let mut stmt = conn.prepare(
        "SELECT year, month, COUNT(*) as count
         FROM media_items
         GROUP BY year, month
         ORDER BY year DESC, month DESC",
    )?;
    let groups = stmt.query_map([], |row| {
        Ok(MonthGroup {
            year: row.get(0)?,
            month: row.get(1)?,
            count: row.get(2)?,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(groups)
}

pub fn get_items_needing_thumbnails(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, path FROM media_items WHERE thumb_ready = 0 ORDER BY media_date DESC",
    )?;
    let items = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
}

pub fn get_direct_children_in_dir(
    conn: &Connection,
    dir: &str,
) -> Result<Vec<(i64, String, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, thumb_path FROM media_items
         WHERE path LIKE ?1 || '/%'
           AND INSTR(SUBSTR(path, LENGTH(?1) + 2), '/') = 0",
    )?;
    let rows = stmt
        .query_map(params![dir], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_all_descendants_in_dir(
    conn: &Connection,
    dir: &str,
) -> Result<Vec<(i64, String, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, thumb_path FROM media_items WHERE path LIKE ?1 || '/%'",
    )?;
    let rows = stmt
        .query_map(params![dir], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn delete_items_by_ids(conn: &Connection, ids: &[i64]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut stmt = tx.prepare("DELETE FROM media_items WHERE id = ?1")?;
    let mut deleted = 0usize;
    for id in ids {
        deleted += stmt.execute(params![id])?;
    }
    drop(stmt);
    tx.commit()?;
    Ok(deleted)
}

pub fn delete_scanned_dirs_with_prefix(conn: &Connection, prefix: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM scanned_dirs WHERE path = ?1 OR path LIKE ?1 || '/%'",
        params![prefix],
    )?;
    Ok(())
}

pub fn get_all_dir_mtimes(conn: &Connection) -> Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT path, dir_mtime FROM scanned_dirs")?;
    let entries = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(entries.into_iter().collect())
}

pub fn update_dir_mtimes(conn: &Connection, entries: &[(String, i64)]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO scanned_dirs (path, dir_mtime) VALUES (?1, ?2)
         ON CONFLICT(path) DO UPDATE SET dir_mtime = excluded.dir_mtime",
    )?;
    for (path, mtime) in entries {
        stmt.execute(params![path, mtime])?;
    }
    Ok(())
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaItem> {
    Ok(MediaItem {
        id: row.get(0)?,
        path: row.get(1)?,
        mtime: row.get(2)?,
        media_date: row.get(3)?,
        year: row.get(4)?,
        month: row.get(5)?,
        media_type: row.get(6)?,
        width: row.get(7)?,
        height: row.get(8)?,
        thumb_path: row.get(9)?,
        thumb_ready: row.get::<_, i32>(10)? != 0,
    })
}
