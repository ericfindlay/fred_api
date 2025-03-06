`fred_api` makes requests to [FRED](https://fred.stlouisfed.org/) to download
economic data, caching it to a data-store so that repeated requests to FRED can be
avoided. Requires the ``FRED_API`` environment variable to be set. When requests
are made to FRED or the cache, the response is written into the file `debug.xml` for
convenient debugging.

```text
API_KEY=abcdefghijklmnopqrstuvwxyz123456
```

```no_run
use {
    fred_api::{build_request, capture_fields, send_request},
    sled::Db
};

# tokio_test::block_on(async {
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
let bytes = send_request(&req_spec, req, true, false, &cache).await.unwrap();

// Consult the FRED API docs XML responses to determine the fields to be captured
// from the bytes. The fields must be specified in the same order they appear in the
// response.
let caps: Vec<Vec<Vec<u8>>> = capture_fields("observation", vec!("date", "value"), &bytes);
assert_eq!(caps[0][0], "   ".as_bytes()); // date
assert_eq!(caps[0][1], "   ".as_bytes()); // value
# })
```

### Requirements

1. ``FRED_API`` environment variable has to be set.

2. A directory used to cache requests is required. This directory is specified in the 
``sled::open("../fred_cache/db")`` function, as shown in the code example below.

```text
API_KEY=abcdefghijklmnopqrstuvwxyz123456
```

### Code Example

```no_run
use {
    fred_api::{build_request, capture_fields, send_request},
    sled::Db
};

# tokio_test::block_on(async {
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
let bytes = send_request(&req_spec, req, true, false, &cache).await.unwrap();

// Consult the FRED API docs XML responses to determine the fields to be captured
// from the bytes. The fields must be specified in the same order they appear in the
// response.
let caps: Vec<Vec<Vec<u8>>> = capture_fields("observation", vec!("date", "value"), &bytes);
assert_eq!(caps[0][0], "   ".as_bytes()); // date
assert_eq!(caps[0][1], "   ".as_bytes()); // value
# })
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

/*!
`fred_api` makes requests to [FRED](https://fred.stlouisfed.org/) to download
economic data, caching it to a data-store so that repeated requests to FRED can be
avoided. Requires the ``FRED_API`` environment variable to be set. When requests
are made to FRED or the cache, the response is written into the file `debug.xml` for
convenient debugging.

