#[tokio::test]
fn basic_request() {
    let db: sled::Db = sled::open("/home/eric/fred_cache/db").unwrap();

    let (req_spec, req) = match build_request("series/observations?series_id=CPGRLE01AUQ657N") {
        Ok((req_spec, req)) => (req_spec, req),
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1)
        },
    };

    let bytes = match send_request(&req_spec, req, false, true, &db).await {
        Ok(b) => b,
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1)
        },
    };
}