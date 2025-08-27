//! Test module to explore Salsa thread safety

#[cfg(test)]
mod tests {
    use crate::db::Database;
    use std::thread;

    #[test]
    fn test_database_clone() {
        let db = Database::new();
        let _db2 = db.clone();
        println!("✅ Database can be cloned");
    }

    #[test]
    #[ignore] // This will fail
    fn test_database_send() {
        let db = Database::new();
        let db2 = db.clone();
        
        thread::spawn(move || {
            let _ = db2;
        }).join().unwrap();
    }
}
