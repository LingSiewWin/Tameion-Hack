//! A dropped connection is retried; a refused range is not.

#[test]
fn a_dropped_connection_is_transient_and_a_refused_range_is_not() {
    use arc::transient_transport_error as t;
    assert!(t(
        "error sending request for url (https://rpc.testnet.arc.network/)"
    ));
    assert!(t("operation timed out"));
    assert!(t("Connection reset by peer"));
    assert!(!t("query returned more than 10000 results"));
    assert!(!t("requested range too large"));
    assert!(!t("rate limit exceeded"));
}
