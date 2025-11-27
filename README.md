`fred_api` makes requests to [FRED](https://fred.stlouisfed.org/) to download
economic data, caching it to a data-store so that repeated requests to FRED can be
avoided.

### Requirements

1. ``FRED_API_KEY`` environment variable can be set or supplied directly.

2. ``FRED_CACHE`` is the directory to place the cache. It can also be set as an
environment variable or supplied directly.

### Code Example

```rust
use fred_api::*;
use sled::{Db, IVec};
use tokio;

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
