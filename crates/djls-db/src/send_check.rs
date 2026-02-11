#[cfg(test)]
mod tests {
    fn assert_send<T: Send>() {}

    #[test]
    fn db_is_send() {
        assert_send::<crate::DjangoDatabase>();
    }

    // DjangoDatabase is intentionally !Sync — salsa::Storage uses RefCell
    // internally. Parallel work uses db.clone() per rayon task instead.
}
