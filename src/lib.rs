/*!
`fred_api` makes requests to [FRED](https://fred.stlouisfed.org/) to download
economic data, caching it to a data-store so that repeated requests to FRED can be
avoided. Requires the ``FRED_API_KEY`` environment variable to be set. When requests
are made to FRED or the cache, the response is written into the file `debug.xml` for
convenient debugging.

### Requirements

1. ``FRED_API_KEY`` environment variable has to be set.

2. A directory used to cache requests is required. This directory is specified in the 
``sled::open("../fred_cache/db")`` function, as shown in the code example below.

A key can be obtained by creating an account at [FRED](https://fredaccount.stlouisfed.org/login/secure/)
, and should be set as an environment variable like
```text
FRED_API_KEY=abcdefghijklmnopqrstuvwxyz123456
```

``Cargo.toml`` should have the following dependences,

```
fred_api = "0.1.4"
sled = "0.34.7"
tokio = "1.44.0"
```

### Code Example

```no_run
use {
    fred_api::{build_request, capture_fields, send_request},
    sled::Db,
    tokio,
};

#[tokio::main]
pub async fn main() {
    // Set the path to the cache. The cache stores the response bytes as is, keyed by
    // the `RequestSpec`.
    let cache: sled::Db = sled::open("../fred_cache/db").unwrap();

    // The spec string can be taken from the FRED API docs. `req_spec` uniquely
    // identifies the request and is used as a key in the cache. `req` is a
    // `hyper::Request`.
    let (req_spec, req) = build_request("series/observations?series_id=CPGRLE01AUQ657N").unwrap();

    // The boolean values determine whether to request from the cache and/or request from
    // FRED. Successful FRED responses are always cached. Responses with other than HTTP
    // status `Ok` status will return an error.
    let bytes = send_request(&req_spec, req, true, true, &cache).await.unwrap();

    let caps: Vec<Vec<Vec<u8>>> = capture_fields("observation", vec!("date", "value"), &bytes);

    println!("{:?}", String::from_utf8(caps[0][0].clone()));
    println!("{:?}", String::from_utf8(caps[0][1].clone()));

    assert_eq!(Ok("1971-04-01"), String::from_utf8(caps[0][0].clone()).as_deref());
    assert_eq!(Ok("0.850603488248666"), String::from_utf8(caps[0][1].clone()).as_deref());
}
```

### Minor Details 

Requests are concurrent and there is no rate limit on the requests. Hopefully making
use of the cache rather than repeatedly making requests for the same data is sufficient
to prevent FRED from blocking excessive requests.

`capture_fields()` uses regular expressions that assume the data is "well-formed" and
the fields are consistently ordered. I haven't come across any problems in my own
use, but I haven't tried to make these expressions robust in any way. There is always
the option of just taking the bytes as is and doing one's own deserialization (which is
recommended in any case because everyone's reliability/simplicity trade-offs are different).
*/

use {
    http::Response,
    hyper::{Body, Client, Request, StatusCode},
    hyper::client::HttpConnector,
    hyper_tls::HttpsConnector,
    regex::bytes::{CaptureMatches, Regex},
    sled::{Db, IVec},
    std::{fmt, fs, env},
};

static BASE_URI: &'static str = "https://api.stlouisfed.org/fred";

#[macro_export]
macro_rules! src {
    {} => 
    { $crate::src(
        option_env!("CARGO_PKG_NAME").map(|s| s.to_string()),
        file!(),
        line!(),
    )};
}

fn src (
    crate_name: Option<String>,
    file: &str,
    line: u32) -> String 
{
    match &crate_name {
        Some(crate_name) => format!("[{}:{}:{}]", crate_name, file, line),
        None => format!("[unknown:{}:{}]", file, line),
    }
}

/**
Build a request from the string specification.

See the [FRED API documentation](https://fred.stlouisfed.org/docs/api/fred/) to build
requests. In these docs each request category has an example XML request such as

`https://api.stlouisfed.org/fred/series/observations?series_id=GNPCA&api_key=abcdefghijklmnopqrstuvwxyz123456`

Extract the middle-part from this, ignoring 

`https://api.stlouisfed.org/fred/`

at the start and 

`&api_key=abcdefghijklmnopqrstuvwxyz123456`

 at the end. For example
```ignore
let (req_spec, req) = request("series/observations?series_id=GNPCA").unwrap();
```
*/
pub fn build_request(mid_part: &str) -> Result<(RequestSpec, Request<Body>), String> {
    let req_spec = RequestSpec::new(mid_part);
    let req_builder: http::request::Builder = Request::builder();
    let uri = req_spec.uri()?;
    match req_builder.method("GET").uri(uri).body(Body::empty()) {
        Ok(req) => Ok((req_spec, req)),
        Err(err) => {
            let msg = format!("{} {}", src!(), err);
            Err(msg)
        },
    }
}

/**
Make a request from cache.
*/
fn cache_request(req_spec: &RequestSpec, db: &Db) -> Result<Option<Vec<u8>>, String> {
    let key: IVec = req_spec.clone().into();
    let ivec = match db.get(key) {
        Ok(Some(ivec)) => ivec,
        Ok(None) => {
            if let Err(err) = fs::write( "debug.xml", format!("{} not in cache", req_spec)) {
                let msg = format!("{} {}", src!(), err);
                return Err(msg)
            };
            return Ok(None)
        },
        Err(err) => {
            let msg = format!("{} {}", src!(), err);
            return Err(msg)
        },
    };
    let bytes: Vec<u8> = ivec.to_vec();
    if let Err(err) = fs::write("debug.xml", &bytes) {
        let msg = format!("{} {}", src!(), err);
        return Err(msg)
    };
    return Ok(Some(bytes));
}

/**
Make a request from FRED. FRED requests always update the cache. A `RequestSpec` is
included as an argument so that it can be used as a key to store a successful request
in the cache.
*/
async fn fred_request(
    req_spec: &RequestSpec,
    req: Request<Body>,
    db: &Db) -> Result<Vec<u8>, String> 
{
    let client_builder: hyper::client::Builder = Client::builder();
    let mut https_connector: HttpsConnector<HttpConnector> = HttpsConnector::new();
    https_connector.https_only(true);
    let client: Client<HttpsConnector<HttpConnector>, Body> = 
        client_builder.build(https_connector);
    // req is of type Request<Body>
    //
    let resp: Response<Body> = match client.request(req).await {
        Ok(r) => r,
        Err(err) => {
            return Err(format!("{} {}", src!(), err))
        },
    };
    let status_code = resp.status();
    match resp.status() {
        StatusCode::OK => {
            let bytes: Vec<u8> = match hyper::body::to_bytes(resp).await {
                Ok(b) => b.to_vec(),
                Err(err) => {
                    let msg = format!("{} {}", src!(), err);
                    return Err(msg)
                },
            };
            if let Err(err) = fs::write("debug.xml", &bytes) {
                let msg = format!("{} {}", src!(), err);
                return Err(msg)
            };
            write_to_cache(req_spec, &bytes.clone(), db)?;
            Ok(bytes)
        }
        _ => Err(format!("{} {}", src!(), status_code)),
    }
}

/**
Send a request, specifying if the request should go to the cache first, and if the
request should go to FRED. Successful requests to FRED are always cached.
```no_run
use fred_api::{build_request, send_request};

# tokio_test::block_on(async {
let db: sled::Db = sled::open("../fred_cache/db").unwrap();
let (req_spec, req) = build_request("series/observations?series_id=CPGRLE01AUQ657N").unwrap();
// request to cache is `true`
// request to FRED is `false`
let bytes = send_request(&req_spec, req, true, false, &db).await.unwrap();
# })
```
*/
pub async fn send_request(
    req_spec: &RequestSpec,
    req: Request<Body>,
    cache: bool,
    fred: bool,
    db: &Db) -> Result<Vec<u8>, String> 
{
    match (cache, fred) {
        (true, true) => {
            match cache_request(req_spec, db) {
                Ok(None) => {
                    return fred_request(req_spec, req, db).await
                },
                Ok(Some(bytes)) => return Ok(bytes),
                Err(e) => return Err(e),
            }
        },
        (true, false) => {
            match cache_request(req_spec, db) {
                Ok(Some(bytes)) => Ok(bytes),
                Ok(None) => Err("Cache only request failed.".to_string()),
                Err(e) => Err(e),
            }
        },
        (false, true) => {
            match fred_request(req_spec, req, db).await {
                Ok(bytes) => Ok(bytes),
                Err(e) => Err(e),
            }
        },
        _ => Err(src!()),
    }
}

/*
Write a FRED response into the caching database.
*/
fn write_to_cache(req_spec: &RequestSpec, bytes: &[u8], db: &Db) -> Result<(), String> {
    let key: IVec = req_spec.clone().into();
    let value: IVec = bytes.as_ref().into();
    match db.contains_key(&key) {
        Ok(true) => {},
        Ok(false) => {
            if let Err(err) = db.insert(key, value) {
                let msg = format!("{} {}", src!(), err);
                return Err(msg)
            }
        },
        Err(err) => {
            let msg = format!("{} {}", src!(), err);
            return Err(msg)
        },
    }
    Ok(())
}

/**
A request spec is the middle-part of a FRED request Uri with the base part removed
from the left and the API key removed from the right.
*/
#[derive(Clone, Debug)]
pub struct RequestSpec(String);

impl RequestSpec {
    pub fn new(mid_part: &str) -> Self {
        RequestSpec(String::from(mid_part))
    }

    pub fn uri(&self) -> Result<String, String> {
        let key = match env::var("FRED_API_KEY") {
            Ok(key) => key,
            Err(err) => {
                let msg = format!("{} The FRED_API_KEY environment variable must be set. {}", src!(), err);
                return Err(msg)
            },
        };
        Ok(format!("{}/{}&api_key={}", BASE_URI, self.0, key))
    }
}

impl fmt::Display for RequestSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
}

impl Into<IVec> for RequestSpec {
    fn into(self) -> IVec { self.0.as_bytes().into() }
}

/**
Return a `Vec` of a `Vec` of bytes extracted from response bytes. The
fields must be in the order they are found in the response.
*/
pub fn capture_fields(
    // "<observation field1="value"...>" The first tags here is 'observation'.
    first_tag: &str,
    fields: Vec<&str>,
    bytes: &[u8]) -> Vec<Vec<Vec<u8>>>
{
    let spacer = "[^>]*";

    let mut re_str = format!("<{} {}", first_tag, spacer);

    for (i, field_name) in fields.iter().enumerate() {

        let field_cap = &format!(r#"{}="([^"]*)""#, field_name);
        re_str.push_str(field_cap);

        if i < fields.len() { re_str.push_str(spacer) }
    }
    re_str.push('>');

    let re = Regex::new(&re_str).unwrap();
    let caps: CaptureMatches = re.captures_iter(&bytes);

    let mut acc = Vec::new();
    for cap in caps {
        let mut field_vec = Vec::new();
        for field_idx in 1..=fields.len() {
            let bytes = &cap[field_idx];
            field_vec.push(bytes.to_vec());
        }
        acc.push(field_vec)
    }
    acc
}

#[test]
fn build_fields_regex_step_1() {

    let bytes: Vec<u8> = r#"<?xml version="1.0" encoding="utf-8" ?>
<seriess realtime_start="2023-11-30" realtime_end="2023-11-30" order_by="series_id" sort_order="asc" count="157" offset="0" limit="1000">
  <series id="AUSUR24NAA" realtime_start="2023-11-30" realtime_end="2023-11-30" title="Adjusted Unemployment Rate for Persons Ages 20 to 24 in Australia (DISCONTINUED)" observation_start="1978-01-01" observation_end="2012-01-01" frequency="Annual" frequency_short="A" units="Percent" units_short="%" seasonal_adjustment="Not Seasonally Adjusted" seasonal_adjustment_short="NSA" last_updated="2013-06-10 09:17:22-05" popularity="0" group_popularity="0" notes="Bureau of Labor Statistics (BLS) has eliminated the International Labor Comparisons (ILC) program. This is the last BLS release of international comparisons of annual labor force statistics."/>
  <series id="AUSURAMS" realtime_start="2023-11-30" realtime_end="2023-11-30" title="Adjusted Unemployment Rate in Australia (DISCONTINUED)" observation_start="2007-01-01" observation_end="2013-06-01" frequency="Monthly" frequency_short="M" units="Percent" units_short="%" seasonal_adjustment="Seasonally Adjusted" seasonal_adjustment_short="SA" last_updated="2013-09-03 11:06:02-05" popularity="1" group_popularity="1" notes="Series is adjusted to U.S. Concepts.
Bureau of Labor Statistics (BLS) has eliminated the International Labor Comparisons (ILC) program. This is the last BLS release of international unemployment rates and employment indexes."/>
</seriess>"#.as_bytes().into();

    let re_init = "<series [^>]*>";
    let re = Regex::new(re_init).unwrap();
    let iter = re.find_iter(&bytes);
    let v: Vec<()> = iter.map(|_| ()).collect();
    assert_eq!(v.len(), 2); // two matches
}

#[test]
fn build_fields_regex_step_2() {

    let bytes: Vec<u8> = r#"<?xml version="1.0" encoding="utf-8" ?>
<seriess realtime_start="2023-11-30" realtime_end="2023-11-30" order_by="series_id" sort_order="asc" count="157" offset="0" limit="1000">
  <series id="AUSUR24NAA" realtime_start="2023-11-30" realtime_end="2023-11-30" title="Adjusted Unemployment Rate for Persons Ages 20 to 24 in Australia (DISCONTINUED)" observation_start="1978-01-01" observation_end="2012-01-01" frequency="Annual" frequency_short="A" units="Percent" units_short="%" seasonal_adjustment="Not Seasonally Adjusted" seasonal_adjustment_short="NSA" last_updated="2013-06-10 09:17:22-05" popularity="0" group_popularity="0" notes="Bureau of Labor Statistics (BLS) has eliminated the International Labor Comparisons (ILC) program. This is the last BLS release of international comparisons of annual labor force statistics."/>
  <series id="AUSURAMS" realtime_start="2023-11-30" realtime_end="2023-11-30" title="Adjusted Unemployment Rate in Australia (DISCONTINUED)" observation_start="2007-01-01" observation_end="2013-06-01" frequency="Monthly" frequency_short="M" units="Percent" units_short="%" seasonal_adjustment="Seasonally Adjusted" seasonal_adjustment_short="SA" last_updated="2013-09-03 11:06:02-05" popularity="1" group_popularity="1" notes="Series is adjusted to U.S. Concepts.
Bureau of Labor Statistics (BLS) has eliminated the International Labor Comparisons (ILC) program. This is the last BLS release of international unemployment rates and employment indexes."/>
</seriess>"#.as_bytes().into();

    let spacer = "[^>]*";
    let field_cap = &format!(r#"{}="([^"]*)"#, "id");

    let re_init = &format!(r#"<{} {}{}{}>"#, "series", spacer, field_cap, spacer);

    // let re_init = r#"<series [^>]*id="([^"]*)"[^>]*>"#;
    let re = Regex::new(re_init).unwrap();
    let mut iter = re.captures_iter(&bytes);
    assert_eq!(&iter.next().unwrap()[1], "AUSUR24NAA".as_bytes());
    assert_eq!(&iter.next().unwrap()[1], "AUSURAMS".as_bytes());
}

#[test]
fn build_fields_regex_step_3() {

    let bytes: Vec<u8> = r#"<?xml version="1.0" encoding="utf-8" ?>
<seriess realtime_start="2023-11-30" realtime_end="2023-11-30" order_by="series_id" sort_order="asc" count="157" offset="0" limit="1000">
  <series id="AUSUR24NAA" realtime_start="2023-11-30" realtime_end="2023-11-30" title="Adjusted Unemployment Rate for Persons Ages 20 to 24 in Australia (DISCONTINUED)" observation_start="1978-01-01" observation_end="2012-01-01" frequency="Annual" frequency_short="A" units="Percent" units_short="%" seasonal_adjustment="Not Seasonally Adjusted" seasonal_adjustment_short="NSA" last_updated="2013-06-10 09:17:22-05" popularity="0" group_popularity="0" notes="Bureau of Labor Statistics (BLS) has eliminated the International Labor Comparisons (ILC) program. This is the last BLS release of international comparisons of annual labor force statistics."/>
  <series id="AUSURAMS" realtime_start="2023-11-30" realtime_end="2023-11-30" title="Adjusted Unemployment Rate in Australia (DISCONTINUED)" observation_start="2007-01-01" observation_end="2013-06-01" frequency="Monthly" frequency_short="M" units="Percent" units_short="%" seasonal_adjustment="Seasonally Adjusted" seasonal_adjustment_short="SA" last_updated="2013-09-03 11:06:02-05" popularity="1" group_popularity="1" notes="Series is adjusted to U.S. Concepts.
Bureau of Labor Statistics (BLS) has eliminated the International Labor Comparisons (ILC) program. This is the last BLS release of international unemployment rates and employment indexes."/>
</seriess>"#.as_bytes().into();

    let caps = capture_fields("series", vec!("id"), &bytes);
    assert_eq!(caps[0], vec!("AUSUR24NAA".as_bytes()));
    assert_eq!(caps[1], vec!("AUSURAMS".as_bytes()));
}
