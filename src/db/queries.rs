use anyhow::Result;
use rusqlite::{params, Connection};

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
        "SELECT id, path FROM media_items WHERE thumb_ready = 0 AND media_type = 'image' ORDER BY media_date DESC",
    )?;
    let items = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
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
