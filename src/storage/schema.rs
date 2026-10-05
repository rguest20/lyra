use super::*;

mod early;
mod middle;
mod recent;

impl LearningStore {
    pub(super) fn migrate(&mut self) -> io::Result<()> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(database_error)?;
        if version > SCHEMA_VERSION {
            return Err(invalid("learning database was created by a newer version"));
        }
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        self.migrate_early(version)?;
        self.migrate_middle(version)?;
        self.migrate_recent(version)?;
        Ok(())
    }
}
