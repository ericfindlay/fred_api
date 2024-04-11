use {
    fred_api::{build_request, send_request},
};

// Test requires FRED_API_KEY to be set.
#[tokio::test]
async fn make_request_to_fred() {

    match build_request("series/observations?series_id=CPGRLE01AUQ657N") {
        Ok((req_spec, req)) => {
            let db: sled::Db = sled::open("/home/eric/fred_cache/db").unwrap();
            let bytes = send_request(&req_spec, req, false, true, &db).await.unwrap();
            assert_eq!(
                &bytes[0..20],
                [60, 63, 120, 109, 108, 32, 118, 101, 114, 115, 105, 111, 110, 61, 34, 49, 46, 48, 34, 32 ],
            );
        }
        Err(err) => assert!(false),
    };

}