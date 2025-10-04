use rusqlite::{Connection, Result};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use roles_logic_sv2::common_messages_sv2::has_work_selection;

pub struct StatsDatabase {
    conn: Mutex<Connection>,
}

impl StatsDatabase {
    pub fn new(path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let conn = Connection::open(path)?;
        let db = StatsDatabase {
            conn: Mutex::new(conn),
        };

        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Hashrate samples table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS hashrate_samples (
                timestamp INTEGER NOT NULL,
                downstream_id INTEGER NOT NULL,
                shares_5min INTEGER NOT NULL,
                estimated_hashrate REAL NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_hashrate_time ON hashrate_samples(timestamp)",
            [],
        )?;

        // Quote history table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS quote_history (
                timestamp INTEGER NOT NULL,
                downstream_id INTEGER NOT NULL,
                amount INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_quote_time ON quote_history(timestamp)",
            [],
        )?;

        // Current stats table (latest snapshot)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS current_stats (
                downstream_id INTEGER PRIMARY KEY,
                shares_submitted INTEGER NOT NULL,
                quotes_created INTEGER NOT NULL,
                ehash_mined INTEGER NOT NULL,
                channels TEXT NOT NULL,
                last_share_time INTEGER,
                connected_at INTEGER NOT NULL,
                is_work_selection_enabled INTEGER NOT NULL,
                address TEXT NOT NULL DEFAULT '-',
                service_type TEXT
            )",
            [],
        )?;

        // Add columns if they don't exist (migration)
        conn.execute(
            "ALTER TABLE current_stats ADD COLUMN address TEXT NOT NULL DEFAULT '-'",
            [],
        ).ok(); // Ignore error if column already exists

        conn.execute(
            "ALTER TABLE current_stats ADD COLUMN service_type TEXT",
            [],
        ).ok(); // Ignore error if column already exists

        Ok(())
    }

    pub fn record_share(&self, downstream_id: u32, timestamp: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Update current stats
        conn.execute(
            "INSERT INTO current_stats (downstream_id, shares_submitted, quotes_created, ehash_mined, channels, last_share_time, connected_at, is_work_selection_enabled)
             VALUES (?1, 1, 0, 0, '[]', ?2, ?2, 0)
             ON CONFLICT(downstream_id) DO UPDATE SET
                shares_submitted = shares_submitted + 1,
                last_share_time = ?2",
            rusqlite::params![downstream_id, timestamp as i64],
        )?;

        Ok(())
    }

    pub fn record_quote(&self, downstream_id: u32, amount: u64, timestamp: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Insert into quote history
        conn.execute(
            "INSERT INTO quote_history (timestamp, downstream_id, amount) VALUES (?1, ?2, ?3)",
            rusqlite::params![timestamp as i64, downstream_id, amount as i64],
        )?;

        // Update current stats
        conn.execute(
            "INSERT INTO current_stats (downstream_id, shares_submitted, quotes_created, ehash_mined, channels, last_share_time, connected_at, is_work_selection_enabled)
             VALUES (?1, 0, 1, ?2, '[]', ?3, ?3, 0)
             ON CONFLICT(downstream_id) DO UPDATE SET
                quotes_created = quotes_created + 1,
                ehash_mined = ehash_mined + ?2",
            rusqlite::params![downstream_id, amount as i64, timestamp as i64],
        )?;

        Ok(())
    }

    pub fn record_channel_opened(&self, downstream_id: u32, channel_id: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Get current channels
        let mut stmt = conn.prepare("SELECT channels FROM current_stats WHERE downstream_id = ?1")?;
        let channels_json: Option<String> = stmt
            .query_row([downstream_id], |row| row.get(0))
            .ok();

        let mut channels: Vec<u32> = channels_json
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        if !channels.contains(&channel_id) {
            channels.push(channel_id);
        }

        let channels_str = serde_json::to_string(&channels).unwrap();

        // Update channels
        conn.execute(
            "INSERT INTO current_stats (downstream_id, shares_submitted, quotes_created, ehash_mined, channels, connected_at, is_work_selection_enabled)
             VALUES (?1, 0, 0, 0, ?2, ?3, 0)
             ON CONFLICT(downstream_id) DO UPDATE SET channels = ?2",
            rusqlite::params![
                downstream_id,
                channels_str,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
            ],
        )?;

        Ok(())
    }

    pub fn record_channel_closed(&self, downstream_id: u32, channel_id: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Get current channels
        let mut stmt = conn.prepare("SELECT channels FROM current_stats WHERE downstream_id = ?1")?;
        let channels_json: Option<String> = stmt
            .query_row([downstream_id], |row| row.get(0))
            .ok();

        let mut channels: Vec<u32> = channels_json
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        channels.retain(|&c| c != channel_id);

        let channels_str = serde_json::to_string(&channels).unwrap();

        // Update channels
        conn.execute(
            "UPDATE current_stats SET channels = ?1 WHERE downstream_id = ?2",
            rusqlite::params![channels_str, downstream_id],
        )?;

        Ok(())
    }

    pub fn record_downstream_connected(&self, downstream_id: u32, flags: u32, address: &str, service_type: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        let is_work_selection = has_work_selection(flags);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        conn.execute(
            "INSERT INTO current_stats (downstream_id, shares_submitted, quotes_created, ehash_mined, channels, connected_at, is_work_selection_enabled, address, service_type)
             VALUES (?1, 0, 0, 0, '[]', ?2, ?3, ?4, ?5)
             ON CONFLICT(downstream_id) DO UPDATE SET
                connected_at = ?2,
                is_work_selection_enabled = ?3,
                address = ?4,
                service_type = ?5",
            rusqlite::params![downstream_id, now, is_work_selection as i64, address, service_type],
        )?;

        Ok(())
    }

    pub fn record_downstream_disconnected(&self, downstream_id: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Remove from current stats
        conn.execute(
            "DELETE FROM current_stats WHERE downstream_id = ?1",
            [downstream_id],
        )?;

        Ok(())
    }

    /// Remove stale miners that haven't sent shares in X seconds
    pub fn cleanup_stale_miners(&self, stale_threshold_secs: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let cutoff_time = now - stale_threshold_secs;

        let removed = conn.execute(
            "DELETE FROM current_stats WHERE last_share_time < ?1 OR (last_share_time IS NULL AND connected_at < ?1)",
            [cutoff_time],
        )?;

        Ok(removed)
    }

    pub fn record_pool_info(&self, listen_address: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Create or update pool_info table with a single row
        conn.execute(
            "CREATE TABLE IF NOT EXISTS pool_info (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                listen_address TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "INSERT INTO pool_info (id, listen_address) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET listen_address = ?1",
            [listen_address],
        )?;

        Ok(())
    }

    pub fn get_pool_address(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();

        match conn.query_row(
            "SELECT listen_address FROM pool_info WHERE id = 1",
            [],
            |row| row.get(0)
        ) {
            Ok(addr) => Ok(Some(addr)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn get_current_stats(&self) -> Result<Vec<DownstreamStats>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT downstream_id, shares_submitted, quotes_created, ehash_mined, channels, last_share_time, connected_at, is_work_selection_enabled, address, service_type
             FROM current_stats"
        )?;

        let stats = stmt
            .query_map([], |row| {
                Ok(DownstreamStats {
                    downstream_id: row.get(0)?,
                    shares_submitted: row.get(1)?,
                    quotes_created: row.get(2)?,
                    ehash_mined: row.get(3)?,
                    channels: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                    last_share_time: row.get(5)?,
                    connected_at: row.get(6)?,
                    is_work_selection_enabled: row.get::<_, i64>(7)? != 0,
                    address: row.get(8)?,
                    service_type: row.get(9)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(stats)
    }

    pub fn get_hashrate_history(&self, hours: i64) -> Result<Vec<HashratePoint>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - (hours * 3600);

        let mut stmt = conn.prepare(
            "SELECT timestamp, downstream_id, estimated_hashrate
             FROM hashrate_samples
             WHERE timestamp > ?1
             ORDER BY timestamp ASC",
        )?;

        let points = stmt
            .query_map([cutoff], |row| {
                Ok(HashratePoint {
                    timestamp: row.get(0)?,
                    downstream_id: row.get(1)?,
                    hashrate: row.get(2)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(points)
    }
}

#[derive(Debug, serde::Serialize)]
pub struct DownstreamStats {
    pub downstream_id: u32,
    pub shares_submitted: u64,
    pub quotes_created: u64,
    pub ehash_mined: u64,
    pub channels: Vec<u32>,
    pub last_share_time: Option<i64>,
    pub connected_at: i64,
    pub is_work_selection_enabled: bool,
    pub address: String,
    pub service_type: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct HashratePoint {
    pub timestamp: i64,
    pub downstream_id: u32,
    pub hashrate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn create_test_db() -> StatsDatabase {
        // Create in-memory database for testing
        let conn = Connection::open_in_memory().unwrap();
        let db = StatsDatabase {
            conn: Mutex::new(conn),
        };
        db.init_schema().unwrap();
        db
    }

    #[test]
    fn test_record_share() {
        let db = create_test_db();
        let downstream_id = 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Record first share
        db.record_share(downstream_id, timestamp).unwrap();

        // Verify stats
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].downstream_id, downstream_id);
        assert_eq!(stats[0].shares_submitted, 1);
        assert_eq!(stats[0].last_share_time, Some(timestamp as i64));

        // Record second share
        db.record_share(downstream_id, timestamp + 10).unwrap();

        // Verify increment
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats[0].shares_submitted, 2);
        assert_eq!(stats[0].last_share_time, Some((timestamp + 10) as i64));
    }

    #[test]
    fn test_record_quote() {
        let db = create_test_db();
        let downstream_id = 1;
        let amount = 1000;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Record first quote
        db.record_quote(downstream_id, amount, timestamp).unwrap();

        // Verify stats
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].downstream_id, downstream_id);
        assert_eq!(stats[0].quotes_created, 1);
        assert_eq!(stats[0].ehash_mined, amount);

        // Record second quote
        db.record_quote(downstream_id, amount * 2, timestamp + 10)
            .unwrap();

        // Verify increment
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats[0].quotes_created, 2);
        assert_eq!(stats[0].ehash_mined, amount + amount * 2);
    }

    #[test]
    fn test_record_channel_opened() {
        let db = create_test_db();
        let downstream_id = 1;
        let channel_id = 100;

        // Open first channel
        db.record_channel_opened(downstream_id, channel_id)
            .unwrap();

        // Verify stats
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].channels, vec![channel_id]);

        // Open second channel
        db.record_channel_opened(downstream_id, channel_id + 1)
            .unwrap();

        // Verify both channels
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats[0].channels, vec![channel_id, channel_id + 1]);

        // Try opening same channel again (should not duplicate)
        db.record_channel_opened(downstream_id, channel_id)
            .unwrap();

        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats[0].channels, vec![channel_id, channel_id + 1]);
    }

    #[test]
    fn test_record_channel_closed() {
        let db = create_test_db();
        let downstream_id = 1;
        let channel_id_1 = 100;
        let channel_id_2 = 101;

        // Open two channels
        db.record_channel_opened(downstream_id, channel_id_1)
            .unwrap();
        db.record_channel_opened(downstream_id, channel_id_2)
            .unwrap();

        // Close first channel
        db.record_channel_closed(downstream_id, channel_id_1)
            .unwrap();

        // Verify only second channel remains
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats[0].channels, vec![channel_id_2]);

        // Close second channel
        db.record_channel_closed(downstream_id, channel_id_2)
            .unwrap();

        // Verify no channels remain
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats[0].channels, Vec::<u32>::new());
    }

    #[test]
    fn test_record_downstream_connected() {
        let db = create_test_db();
        let downstream_id = 1;
        let flags = 1; // Work selection enabled

        db.record_downstream_connected(downstream_id, flags, "127.0.0.1:1234", None)
            .unwrap();

        // Verify stats
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].downstream_id, downstream_id);
        assert!(stats[0].is_work_selection_enabled);

        // Connect with different flags
        let flags_no_work_selection = 0;
        db.record_downstream_connected(downstream_id, flags_no_work_selection, "127.0.0.1:1234", None)
            .unwrap();

        let stats = db.get_current_stats().unwrap();
        assert!(!stats[0].is_work_selection_enabled);
    }

    #[test]
    fn test_record_downstream_disconnected() {
        let db = create_test_db();
        let downstream_id = 1;

        // Connect downstream
        db.record_downstream_connected(downstream_id, 0, "127.0.0.1:1234", None).unwrap();

        // Verify connected
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats.len(), 1);

        // Disconnect
        db.record_downstream_disconnected(downstream_id).unwrap();

        // Verify removed
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats.len(), 0);
    }

    #[test]
    fn test_multiple_downstreams() {
        let db = create_test_db();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Record shares from different downstreams
        db.record_share(1, timestamp).unwrap();
        db.record_share(2, timestamp).unwrap();
        db.record_share(1, timestamp + 1).unwrap();

        // Verify both downstreams tracked separately
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats.len(), 2);

        let downstream_1 = stats.iter().find(|s| s.downstream_id == 1).unwrap();
        let downstream_2 = stats.iter().find(|s| s.downstream_id == 2).unwrap();

        assert_eq!(downstream_1.shares_submitted, 2);
        assert_eq!(downstream_2.shares_submitted, 1);
    }

    #[test]
    fn test_get_hashrate_history_empty() {
        let db = create_test_db();

        // Query with no data
        let points = db.get_hashrate_history(24).unwrap();
        assert_eq!(points.len(), 0);
    }

    #[test]
    fn test_combined_operations() {
        let db = create_test_db();
        let downstream_id = 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Connect
        db.record_downstream_connected(downstream_id, 1, "127.0.0.1:1234", None).unwrap();

        // Open channels
        db.record_channel_opened(downstream_id, 100).unwrap();
        db.record_channel_opened(downstream_id, 101).unwrap();

        // Record shares and quotes
        db.record_share(downstream_id, timestamp).unwrap();
        db.record_quote(downstream_id, 5000, timestamp).unwrap();
        db.record_share(downstream_id, timestamp + 1).unwrap();

        // Verify all stats together
        let stats = db.get_current_stats().unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].downstream_id, downstream_id);
        assert_eq!(stats[0].shares_submitted, 2);
        assert_eq!(stats[0].quotes_created, 1);
        assert_eq!(stats[0].ehash_mined, 5000);
        assert_eq!(stats[0].channels, vec![100, 101]);
        assert!(stats[0].is_work_selection_enabled);
    }
}
