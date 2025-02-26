pub const STORE_INFO_CREATE_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS store_info (
        content_type TEXT PRIMARY KEY,
        version TEXT NOT NULL
    )";

pub const STORE_INFO_UPDATE: &str = "
    INSERT OR REPLACE INTO store_info (content_type, version)
    VALUES (:content_type, :version)";

pub const STORE_INFO_LOOKUP: &str = "
    SELECT version
    FROM store_info
    WHERE content_type = :content_type LIMIT 1";
