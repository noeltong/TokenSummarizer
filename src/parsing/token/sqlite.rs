//! SQLite 打开策略。
//!
//! 远端镜像里的库是本地副本，直接只读打开即可；但 WSL 的 `\\wsl$\...` 走 9P，
//! SQLite 的 WAL / 文件锁在这类网络文件系统上不可靠（可能出现 "database is
//! locked" 或读到不一致快照）。因此：
//!
//! - `Platform::Wsl`：始终把 `db` + `-wal` + `-shm` 复制到本地临时目录，
//!   以**读写**方式打开副本，让 SQLite 自己完成 WAL 恢复；
//! - 其它平台：先尝试只读打开（沿用 ZCode 的既有做法），失败再复制重试。
//!
//! 临时目录由 `Drop` 清理（连接先关闭，再删目录——Windows 上顺序反了会删不掉）。

use std::ops::Deref;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::model::Platform;

/// 打开期间持有临时副本的连接守卫。
pub struct ScopedConnection {
    conn: Option<Connection>,
    temp_dir: Option<PathBuf>,
}

impl Deref for ScopedConnection {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("连接在 Drop 前始终有效")
    }
}

impl Drop for ScopedConnection {
    fn drop(&mut self) {
        // 先关连接：Windows 不允许删除仍被打开的文件。
        self.conn.take();
        if let Some(dir) = self.temp_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// 按平台选择打开方式。
pub fn open_for_scan(db: &Path, platform: Platform) -> Result<ScopedConnection, String> {
    if platform == Platform::Wsl {
        return open_via_copy(db);
    }
    match open_readonly(db) {
        Ok(connection) => Ok(ScopedConnection {
            conn: Some(connection),
            temp_dir: None,
        }),
        // 只读连接无法做 WAL 恢复（恢复需要写权限），复制一份再以读写方式打开。
        Err(_) => open_via_copy(db),
    }
}

fn open_readonly(db: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    let _ = connection.busy_timeout(std::time::Duration::from_millis(3000));
    warm_up(&connection)?;
    Ok(connection)
}

fn open_via_copy(db: &Path) -> Result<ScopedConnection, String> {
    let dir = crate::util::paths::temp_dir().join(format!("db-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
    let copy = dir.join("scan.sqlite3");
    if let Err(err) = copy_bundle(db, &copy) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(err);
    }
    let connection = Connection::open(&copy)
        .map_err(|e| format!("打开 SQLite 副本失败：{e}"))?;
    let _ = connection.busy_timeout(std::time::Duration::from_millis(3000));
    if let Err(err) = warm_up(&connection) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(err);
    }
    Ok(ScopedConnection {
        conn: Some(connection),
        temp_dir: Some(dir),
    })
}

/// 复制 `db` / `-wal` / `-shm` 三件套；`-shm` 丢失无妨（SQLite 会重建）。
fn copy_bundle(db: &Path, dest: &Path) -> Result<(), String> {
    std::fs::copy(db, dest).map_err(|e| format!("复制 SQLite 主库失败：{e}"))?;
    for suffix in ["-wal", "-shm"] {
        let source = PathBuf::from(format!("{}{suffix}", db.display()));
        if source.is_file() {
            let target = PathBuf::from(format!("{}{suffix}", dest.display()));
            // side 文件复制失败不致命：主库仍然可读，只是可能少最新事务。
            let _ = std::fs::copy(&source, &target);
        }
    }
    Ok(())
}

fn warm_up(connection: &Connection) -> Result<(), String> {
    connection
        .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 清理上次异常退出遗留的临时副本（超过 `max_age` 的）。
pub fn cleanup_stale(max_age: std::time::Duration) {
    let root = crate::util::paths::temp_dir();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if now.duration_since(modified).map(|age| age > max_age).unwrap_or(false) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;

    fn make_db(path: &Path) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch("CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (42);")
            .unwrap();
    }

    #[test]
    fn readonly_open_works_on_plain_file() {
        let dir = TempDir::new();
        let db = dir.path().join("a.sqlite3");
        make_db(&db);
        let scoped = open_for_scan(&db, Platform::Linux).expect("只读打开");
        let value: i64 = scoped.query_row("SELECT v FROM t", [], |row| row.get(0)).unwrap();
        assert_eq!(value, 42);
        assert!(scoped.temp_dir.is_none(), "本地文件不应复制");
    }

    #[test]
    fn wsl_always_copies_into_temp() {
        let dir = TempDir::new();
        let db = dir.path().join("b.sqlite3");
        make_db(&db);
        let scoped = open_for_scan(&db, Platform::Wsl).expect("复制打开");
        let value: i64 = scoped.query_row("SELECT v FROM t", [], |row| row.get(0)).unwrap();
        assert_eq!(value, 42);
        let temp = scoped.temp_dir.clone().expect("WSL 必须走复制路径");
        assert!(temp.exists());
        drop(scoped);
        assert!(!temp.exists(), "Drop 后临时副本必须清理");
    }

    #[test]
    fn wal_content_is_visible_after_copy() {
        let dir = TempDir::new();
        let db = dir.path().join("c.sqlite3");
        let writer = Connection::open(&db).unwrap();
        writer
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
            .unwrap();
        writer
            .execute_batch(
                "PRAGMA wal_autocheckpoint=0; CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (7);",
            )
            .unwrap();
        assert!(
            PathBuf::from(format!("{}-wal", db.display())).exists(),
            "数据应留在未 checkpoint 的 WAL 里"
        );
        // 写连接仍开着：副本必须靠 -wal 才能读到这行
        let scoped = open_via_copy(&db).expect("复制打开");
        let value: i64 = scoped.query_row("SELECT v FROM t", [], |row| row.get(0)).unwrap();
        assert_eq!(value, 7, "副本必须能读到 WAL 里未 checkpoint 的数据");
        drop(scoped);
        drop(writer);
    }

    #[test]
    fn missing_db_reports_error() {
        let dir = TempDir::new();
        let db = dir.path().join("absent.sqlite3");
        assert!(open_for_scan(&db, Platform::Wsl).is_err());
    }
}
