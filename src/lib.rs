/*!
`fred_api` makes requests to [FRED](https://fred.stlouisfed.org/) to download
economic data, caching it to a data-store so that repeated requests to FRED can be
avoided.

### Requirements

1. ``FRED_API_KEY`` environment variable can be set or supplied directly.

2. ``FRED_CACHE`` is the directory to place the cache. It can also be set as an
environment variable or supplied directly.

### Code Example

```no_run
use {
    debug_err::{DebugErr, src},
    fred_api::{build_request, FieldIter, fred_cache, Lookup, send_request},
    sled::{Db, IVec},
    tokio,
};

#[tokio::main]
pub async fn main() {
    // Set the path to the cache. The cache stores the response bytes as is, keyed by
    // the `RequestSpec`.
    let cache: sled::Db = sled::open(fred_cache(None).unwrap()).unwrap();

    // See the FRED API documentation to build requests. In these docs each request
    // category has an example XML request such as:
    // 
    // https://api.stlouisfed.org/fred/series/observations?series_id=GNPCA&api_key=abcdefghijklmnopqrstuvwxyz123456
    //                                 ------------------------------------
    // The first function argument is underlined section including the "&" or the "?"
    // preceding "api_key". If the second function argument is None, the API key is read
    // from the environment variable FRED_API_KEY.
    let req = build_request("series/observations?series_id=GNPCA&", None).unwrap();

    // Lookup options are Lookup::FredOnly, Lookup::CacheOnly or
    // Lookup::FredOnCacheMiss. Successful FRED responses are always cached.
    let bytes: IVec = send_request(&req, Lookup::FredOnCacheMiss, &cache).await.unwrap();

    let mut field_iter = FieldIter::new("observation", vec!("date", "value"), bytes);
    let fields = field_iter.next().unwrap().unwrap();
    assert_eq!("1971-04-01", fields[0]);
    assert_eq!(0.850603488248666, fields[1].parse::<f32>().unwrap());
}
```
*/

use {
    http::{StatusCode, uri::Uri},
    http_body_util::{BodyExt, Empty},
    hyper::body::Bytes,
    hyper_util::{client::legacy::Client, rt::TokioExecutor},
    hyper_rustls::ConfigBuilderExt,
    quick_xml::{events::{Event}, reader::Reader},
    rustls::{version::TLS13},
    sled::{Db, IVec},
    std::{fmt, env, io::Cursor, path::PathBuf, str::FromStr},
};

static BASE_URI: &'static str = "https://api.stlouisfed.org/fred";

pub use debug_err::{src, DebugErr};
pub type Result<T> = std::result::Result<T, DebugErr>;

/**
Retrieves ``FRED_CACHE`` environment variable if set or fails.
*/    
pub fn fred_cache(opt_path: Option<&str>) -> Result<PathBuf> {
    match opt_path {
        Some(path) => Ok(PathBuf::from(path)),
        None => {
            env::var("FRED_CACHE")
                .map(PathBuf::from)
                .map_err(|e| src!("Failed to read FRED_CACHE with error: '{e}'."))
        },
    }
}

/**
Build a request from the mid-part of the URL.
```text
https://api.stlouisfed.org/fred/series/observations?series_id=GNPCA&api_key=abcdefghijklmnopqrstuvwxyz123456
                                ------------------------------------
```
If ``FRED_API_KEY`` is set as an environment variable use ``None``.
*/
// No tests required because this function is just very simple window dressing for users.
// test:
pub fn build_request(mid_part: &str, api_key: Option<&str>) -> Result<RequestSpec> {
    RequestSpec::new(mid_part, api_key)
}

/**
Non-async request to cache only.
*/
// test: cache_request_hit_and_miss_works
pub fn cache_request(req: &RequestSpec, db: &Db) -> Result<Option<IVec>> {
    // let key: IVec = req.clone().into();
    let ivec = match db.get(req.ivec()) {
        Ok(Some(ivec)) => ivec,
        Ok(None) => { return Ok(None) },
        // Failed to induce an error in Sled using Linux permissions or disk corruption.
        Err(e) => Err(src!("{e}"))?,
    };
    return Ok(Some(ivec));
}

/**
Request to FRED bypassing cache.
*/
// test: fred_request_should_return_err_on_bad_request
async fn fred_request(req: &RequestSpec, db: &Db) -> Result<IVec> {
    let tls = rustls::ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .with_native_roots().map_err(|e| src!("{e}"))?
        .with_no_client_auth();

    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_only()
        .enable_http2()
        .build();

    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(https);

    let fut = async move {
        let res = client
            .get(req.uri()?)
            .await
            .map_err(|err| src!("Request failed with error: {err}."))?;

        let status = res.status();

        let body = res
            .into_body()
            .collect()
            .await
            .map_err(|err| src!("Could not get body: {err}."))?
            .to_bytes();

        if status == StatusCode::OK {
            write_to_cache(req, body.as_ref(), db)?;
            let ivec = db
                .get(req.ivec())
                .map_err(|e| src!("{e}"))?
                .ok_or(src!("Just inserted but not found"))?;
            Ok(ivec)
        } else {
            let mut field_iter = FieldIter::new("error", vec!["message"], body.as_ref().into())
                .take_while(|result| result.is_ok()).map(|result| result.unwrap());
            
            let message = field_iter.next()
                .and_then(|fields| fields.first().cloned())
                .unwrap_or("Unknown error".to_string());
            Err(src!("FRED API error: '{message}'"))
        }
    };

    fut.await
}

/**
Send a request to FRED or the cache, using the lookup method to determine procedure.
```no_run
use fred_api::{build_request, fred_cache, Lookup, send_request};

# tokio_test::block_on(async {
let db: sled::Db = sled::open(fred_cache(None).unwrap()).unwrap();
let req = build_request("series/observations?series_id=CPGRLE01AUQ657N&", None).unwrap();
let bytes = send_request(&req, Lookup::CacheOnly, &db).await.unwrap();
# })
```
*/
pub async fn send_request(
    req: &RequestSpec,
    lookup: Lookup,
    db: &Db) -> Result<IVec> 
{
    match lookup {
        Lookup::FredOnCacheMiss => {
            match cache_request(req, db) {
                Ok(None) => { return fred_request(req, db).await },
                Ok(Some(bytes)) => return Ok(bytes),
                Err(e) => return Err(e),
            }
        },
        Lookup::CacheOnly => {
            match cache_request(req, db) {
                Ok(Some(bytes)) => Ok(bytes),
                Ok(None) => Err(src!(
                    "Cache only request (mid-part '{}') failed",
                    req.mid_part()
                ))?,
                Err(e) => Err(e),
            }
        },
        Lookup::FredOnly => {
            match fred_request(req, db).await {
                Ok(bytes) => Ok(bytes),
                Err(e) => Err(e),
            }
        },
    }
}

/*
Write a FRED response into the caching database.
*/
fn write_to_cache(req: &RequestSpec, bytes: &[u8], db: &Db) -> Result<()> {
    let key: IVec = req.ivec();
    let value: IVec = bytes.as_ref().into();
    match db.contains_key(&key) {
        Ok(true) => {},
        Ok(false) => { if let Err(err) = db.insert(key, value) { Err(src!("{err}"))? }},
        Err(err) => Err(src!("{err}"))?,
    }
    Ok(())
}

/**
A request spec is the middle-part of a FRED request Uri with the base part removed
from the left and the API key removed from the right.
*/
#[derive(Clone)]
pub struct RequestSpec {
    mid_part: String,
    key: String,
}

impl RequestSpec {

    /**
    If ``api_key`` is ``None``, checks for environment variable ``FRED_API_KEY``, else
    uses the value provided.
    */
    // test: new_request_spec_works
    pub fn new(mid_part: &str, api_key: Option<&str>) -> Result<Self> {
        let key = match api_key {
            Some(key) => key.to_string(),
            None => env::var("FRED_API_KEY")
                .map_err(|err| src!("FRED_API_KEY environment variable missing: {err}"))?,
        };
        Ok(RequestSpec {
            mid_part: mid_part.to_string(),
            key,
        })
    }

    // test: uri_request_spec_edge_case
    pub fn uri(&self) -> Result<Uri> {
        let s = &format!("{}/{}api_key={}", BASE_URI, self.mid_part, self.key);
        Ok(Uri::from_str(s).map_err(|e| src!("{e}"))?)
    }

    pub fn mid_part(&self) -> String { self.mid_part.clone() }

    // test: ivec_as_key
    pub fn ivec(&self) -> IVec {
        self.mid_part.as_bytes().into()
    }

    pub fn has_api_key(&self) -> bool { !self.key.is_empty() }
}

impl fmt::Display for RequestSpec {
    // test: request_spec_hides_api_key    
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.mid_part) }
}

impl std::fmt::Debug for RequestSpec {
    // test: request_spec_hides_api_key    
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let key_len = self.key.len();
        f.debug_struct("RequestSpec")
            .field("mid_part", &self.mid_part)
            .field("key", &format!("({} characters)", key_len))
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct FieldIter {
    reader: Reader<Cursor<IVec>>, // Use Cursor<IVec>
    fields: Vec<String>,
    tag: String,
    buf: Vec<u8>,
    has_errored: bool,
}

impl FieldIter {
    pub fn new(tag: &str, fields: Vec<&str>, ivec: IVec) -> Self {
        let tag = tag.to_string();
        let fields = fields.into_iter().map(|s| s.to_string()).collect();
        let mut reader = Reader::from_reader(Cursor::new(ivec.clone()));
        reader.config_mut().trim_text(true);
        FieldIter {
            reader,
            fields,
            tag,
            buf: Vec::new(),
            has_errored: false,
        }
    }
}

impl Iterator for FieldIter {
    type Item = Result<Vec<String>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.has_errored {
            return None;
        }
        loop {
            self.buf.clear();
            match self.reader.read_event_into(&mut self.buf) {
                Ok(Event::Start(bytes_start)) | Ok(Event::Empty(bytes_start))
                    if bytes_start.name().as_ref() == self.tag.as_bytes() => {
                    let mut row = Vec::with_capacity(self.fields.len());
                    for field in &self.fields {
                        let attr_value = bytes_start
                            .attributes()
                            .filter_map(std::result::Result::ok)
                            .find(|a| a.key.as_ref() == field.as_bytes());

                        match attr_value {
                            Some(a) => match self.reader.decoder().decode(&a.value) {
                                Ok(value) => {
                                    if value.is_empty() {
                                        self.has_errored = true;
                                        return Some(Err(src!(
                                            "Empty attribute '{}' in tag '{}'",
                                            field,
                                            self.tag
                                        )));
                                    }
                                    row.push(value.to_string());
                                }
                                Err(e) => {
                                    self.has_errored = true;
                                    return Some(Err(src!(
                                        "XML decoding error for attribute '{}': {}",
                                        field,
                                        e
                                    )));
                                }
                            },
                            None => {
                                self.has_errored = true;
                                return Some(Err(src!(
                                    "Missing attribute '{}' in tag '{}'",
                                    field,
                                    self.tag
                                )));
                            }
                        }
                    }
                    return Some(Ok(row));
                }
                Ok(Event::Eof) => return None,
                Err(e) => {
                    self.has_errored = true;
                    return Some(Err(src!("XML parsing error: {e}")));
                }
                _ => (),
            }
        }
    }
}

/**
Determines the lookup method. A successful request will always write to the cache.
*/
#[derive(Clone, Copy, Debug)]
pub enum Lookup {
    FredOnCacheMiss,
    FredOnly,
    CacheOnly,
}

impl FromStr for Lookup {
    type Err = DebugErr;
    
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "fred_on_cache_miss" => Ok(Lookup::FredOnCacheMiss),
            "fred_only" => Ok(Lookup::FredOnly),
            "cache_only" => Ok(Lookup::CacheOnly),
            _ => Err(src!("Could not parse '{}'", s))?,
        }
    }
}

#[cfg(test)]
mod test {
    use {
        crate::*,
        lazy_static::lazy_static,
        tempfile::TempDir,
    };

    lazy_static! {
        static ref TEMP_DIR: TempDir = TempDir::new().unwrap();
        static ref DB: Db = sled::open(TEMP_DIR.path()).unwrap();
    }    

    /// Creates a sled database in a temporary directory.
    // test:
    fn create_temp_cache() -> Db {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        sled::open(temp_dir.path()).expect("Failed to open sled database")
    }    

    #[test]
    fn new_request_spec_works() {

        // Ends with API key.
        let api_key = "abcdefghijklmnopqrstuvwxyz123456";
        let req_spec = RequestSpec::new("tags?", Some(api_key)).unwrap();
        assert!(req_spec.uri().unwrap().to_string()
            .ends_with(&format!("api_key={}", api_key))
        );

        // Agree with FRED documentation
        let api_key = "abcd";
        let samples = vec![
            (
                "category?category_id=125&",
                "https://api.stlouisfed.org/fred/category?category_id=125&api_key=abcd",
            ),
            (
                "category/children?category_id=13&",
                "https://api.stlouisfed.org/fred/category/children?category_id=13&api_key=abcd",
            ),
            (
                "category/related?category_id=32073&",
                "https://api.stlouisfed.org/fred/category/related?category_id=32073&api_key=abcd",
            ),
        ];
        for sample in samples {
            assert_eq!(
                RequestSpec::new(sample.0, Some(api_key)).unwrap().uri().unwrap().to_string(),
                sample.1,
            )
        }
    }
    
    #[tokio::test]
    async fn cache_request_hit_and_miss_works() {

        // Set up request
        let api_key = "abcd";
        let req = RequestSpec::new("category?category_id=125&", Some(api_key)).unwrap();
        let response_bytes: &[u8] = b"response_bytes";
        let db = create_temp_cache();

        // Insert a test key-value pair to the test cache.
        let key: IVec = req.ivec();
        let value: IVec = response_bytes.as_ref().into();
        db.insert(key, value).unwrap();

        // Should return value on cache hit.
        assert_eq!(cache_request(&req, &db).unwrap().unwrap(), response_bytes);

        let req = RequestSpec::new("category/children?category_id=13&", Some(api_key)).unwrap();

        // Should return None on cache-miss.
        assert!(cache_request(&req, &db).unwrap().is_none());
    }

    #[test]
    fn uri_request_spec_edge_case() {
        // Space.
        let e = RequestSpec::new("observations?series_id=GNPCA &", Some("abcd"))
            .unwrap()
            .uri()
            .unwrap_err();
        // No space.
        let _ = RequestSpec::new("observations?series_id=GNPCA&", Some("abcd"))
            .unwrap()
            .uri()
            .is_ok();
        assert_eq!(e.msg, "invalid uri character");
    }

    #[test]
    fn ivec_as_key() {
        let ivec: IVec = RequestSpec::new("observations?series_id=GNPCA&", Some("abcd"))
            .unwrap()
            .ivec();
        assert_eq!(b"observations?series_id=GNPCA&", &*ivec);
    }

    #[test]
    fn bytes_from_ivec_are_unchanged() {
        let bytes: Vec<u8> = r#"<?xml version=\"1.0\" encoding=\"utf-8\" ?>\n<observations realtime_start=\"2025-10-04\" realtime_end=\"2025-10-04\" observation_start=\"1600-01-01\" observation_end=\"9999-12-31\" units=\"lin\" output_type=\"1\" file_type=\"xml\" order_by=\"observation_date\" sort_order=\"asc\" count=\"210\" offset=\"0\" limit=\"100000\">\n  <observation realtime_start=\"2025-10-04\" realtime_end=\"2025-10-04\" date=\"1971-04-01\" value=\"0.850603488248666\"/>\n  <observation realtime_start=\"2025-10-04\" realtime_end=\"2025-10-04\" date=\"1971-07-01\" value=\"3.43557210303712\"/>\n  <observation realtime_start=\"2025-10-04\" realtime_end=\"2025-10-04\" date=\"1971-10-01\" value=\"1.90453329926268\"/>\n  <observation realtime_start=\"2025-10-04\" realtime_end=\"2025-10-04\" date=\"1972-01-01\" value=\"0.988357368475625\"/>\n  <observation realtime_start=\"2025-10-04\" realtime_end=\"2025-10-04\" date=\"1972-04-01\" value=\"1.14890574736089\"/>\n  <observation realtime_start=\"2025-10-04\" realtime_end=\"2025-10-04\" date=\"1972-07-01\" value=\"1.84453195549092\"/>\n  <observation realtime_start=\"2025-10-04\" realtime_end=\"2025-10-04\" date=\"1972-10-01\" value=\"0.838712265813754\"/>\n</observations>\n\n\n\n"#.into();
        let ivec: IVec = bytes.clone().into();
        let bytes_from_ivec = &*ivec;
        assert_eq!(bytes.clone(), bytes_from_ivec);
    }

    #[test]
    fn request_spec_hides_api_key() {
        // Debug.
        assert_eq!(
            format!("{:?}", RequestSpec::new("abc", Some("abcdefghijklmnopqrstuvwxyz123456")).unwrap()),
            "RequestSpec { mid_part: \"abc\", key: \"(32 characters)\" }"
        );
        // Display.
        assert_eq!(
            format!("{}", RequestSpec::new("abc", Some("abcdefghijklmnopqrstuvwxyz123456")).unwrap()),
            "abc"
        );
    }

    #[test]
    fn has_api_key_works() {
        let req = RequestSpec::new("observations?series_id=GNPCA&", Some("abcd")).unwrap();
        assert!(req.has_api_key());

        // Requires test for None, that first removes the environment variable, runs
        // the test, and then resets the environment variable.
    }

    #[test]
    fn field_iter_repeats_error_despite_valid_next_line() {
        let mut xml: Vec<u8> = r#"<?xml version="1.0" encoding="utf-8" ?>
    <observations>
        <observation date="1929-01-01" value="1202.659" />
        <observation date=""#.to_string().into_bytes();
        xml.push(0xFF); // Non-UTF-8 byte
        xml.extend(r#"" value="1000.0" />
        <observation date="1930-01-01" value="1000.0" />
    </observations>"#.to_string().into_bytes());

        let mut field_iter = FieldIter::new("observation", vec!["date", "value"], xml.into());

        assert_eq!(
            field_iter.next().unwrap().unwrap(),
            vec!["1929-01-01".to_string(), "1202.659".to_string()],
        );

        if let Err(e) = field_iter.next().unwrap() {
            assert!(e.msg.contains("invalid utf-8"))
        } else {
            assert!(false, "Should fail")
        }

        assert!(field_iter.next().is_none())
    }    

    #[test]
    fn field_iter_errors_on_missing_attribute() {
        let xml: Vec<u8> = r#"<?xml version="1.0" encoding="utf-8" ?>
    <observations>
        <observation date="1929-01-01" value="1202.659" />
        <observation date="1929-02-01" />
        <observation date="1930-01-01" value="1000.0" />
    </observations>"#.to_string().into_bytes();

        let mut field_iter = FieldIter::new("observation", vec!["date", "value"], xml.into());

        assert_eq!(
            field_iter.next().unwrap().unwrap(),
            vec!["1929-01-01".to_string(), "1202.659".to_string()],
        );
    }
}

