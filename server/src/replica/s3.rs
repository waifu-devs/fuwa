//! Just enough of the S3 API for the replica: putting, getting, listing and
//! deleting objects in one bucket, signed with AWS Signature Version 4. Works
//! with any S3-compatible store (Railway buckets, AWS, R2, MinIO…).

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use http::{Method, StatusCode};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};

/// How a bucket is reached, as `FUWA_S3_*` sets it.
#[derive(Clone)]
pub struct S3Config {
    /// FUWA_S3_BUCKET.
    pub bucket: String,
    /// FUWA_S3_ENDPOINT, e.g. `https://t3.storageapi.dev`; default AWS's for the region.
    pub endpoint: String,
    /// FUWA_S3_REGION, default `auto` (what Railway and R2 use).
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// FUWA_S3_PATH_STYLE: `endpoint/bucket/key` instead of `bucket.endpoint/key`.
    pub path_style: bool,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("path_style", &self.path_style)
            .finish_non_exhaustive()
    }
}

/// An object a listing found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub key: String,
    pub size: u64,
}

pub struct S3 {
    config: S3Config,
    /// `https` or `http`.
    scheme: String,
    /// The endpoint's host (and port), without the bucket.
    host: String,
    client: reqwest::Client,
}

/// What small requests (everything but whole files) may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Characters S3 wants percent-encoded in a path: everything but `A-Z a-z 0-9 - _ . ~`.
const ENCODE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

impl S3 {
    pub fn new(config: S3Config) -> Result<Self> {
        let (scheme, rest) = config
            .endpoint
            .split_once("://")
            .filter(|(scheme, _)| matches!(*scheme, "https" | "http"))
            .ok_or_else(|| Error::internal("FUWA_S3_ENDPOINT must start with https://"))?;
        let host = rest.trim_end_matches('/').to_string();
        // No user:password@ either: the host shows up in errors.
        if host.is_empty() || host.contains('/') || host.contains('@') {
            return Err(Error::internal("FUWA_S3_ENDPOINT must be just a scheme and host"));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|err| Error::internal(format!("couldn't start the bucket client: {err}")))?;
        Ok(Self { scheme: scheme.to_string(), host, client, config })
    }

    pub fn bucket(&self) -> &str {
        &self.config.bucket
    }

    pub async fn put(&self, key: &str, body: Bytes) -> Result<()> {
        let hash = hex(&Sha256::digest(&body));
        let length = body.len() as u64;
        let response =
            self.send(Method::PUT, key, &[], &hash, Some(length), Some(reqwest::Body::from(body)), true).await?;
        check(response).await.map(drop)
    }

    /// Uploads a file as it is on disk, streaming it, and returns its size.
    /// The file must not change while it's read.
    pub async fn put_file(&self, key: &str, path: &Path) -> Result<u64> {
        let file = tokio::fs::File::open(path).await?;
        let length = file.metadata().await?.len();
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
        let response = self.send(Method::PUT, key, &[], "UNSIGNED-PAYLOAD", Some(length), Some(body), false).await?;
        check(response).await?;
        Ok(length)
    }

    pub async fn get(&self, key: &str) -> Result<Option<Bytes>> {
        let response = self.send(Method::GET, key, &[], EMPTY_SHA256, None, None, true).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = check(response).await?;
        Ok(Some(response.bytes().await.map_err(|err| Error::internal(format!("bucket: {err}")))?))
    }

    /// Downloads an object into `path` (replacing it). False when there's no such object.
    pub async fn get_to_file(&self, key: &str, path: &Path) -> Result<bool> {
        let response = self.send(Method::GET, key, &[], EMPTY_SHA256, None, None, false).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        let response = check(response).await?;
        let mut out = tokio::fs::File::create(path).await?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            out.write_all(&chunk.map_err(|err| Error::internal(format!("bucket: {err}")))?).await?;
        }
        out.sync_all().await?;
        Ok(true)
    }

    /// Every object whose key starts with `prefix`, in key order.
    pub async fn list(&self, prefix: &str) -> Result<Vec<Object>> {
        let mut objects = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("list-type", "2".to_string()), ("prefix", prefix.to_string())];
            if let Some(token) = &token {
                query.push(("continuation-token", token.clone()));
            }
            let response = self.send(Method::GET, "", &query, EMPTY_SHA256, None, None, true).await?;
            let text = check(response).await?.text().await.map_err(|err| Error::internal(format!("bucket: {err}")))?;
            let page = parse_list(&text)?;
            objects.extend(page.objects);
            match page.next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        objects.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(objects)
    }

    /// Deletes an object; one that isn't there is fine.
    pub async fn delete(&self, key: &str) -> Result<()> {
        let response = self.send(Method::DELETE, key, &[], EMPTY_SHA256, None, None, true).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(response).await.map(drop)
    }

    #[allow(clippy::too_many_arguments)]
    async fn send(
        &self,
        method: Method,
        key: &str,
        query: &[(&str, String)],
        payload_hash: &str,
        length: Option<u64>,
        body: Option<reqwest::Body>,
        small: bool,
    ) -> Result<reqwest::Response> {
        let (host, path) = self.address(key);
        let query = canonical_query(query);
        let signed = sign(
            &Request { method: method.as_str(), host: &host, path: &path, query: &query, payload_hash, extra: &[] },
            &self.config,
            SystemTime::now(),
        );
        let url = if query.is_empty() {
            format!("{}://{host}{path}", self.scheme)
        } else {
            format!("{}://{host}{path}?{query}", self.scheme)
        };
        let mut request = self
            .client
            .request(method, url)
            .header("x-amz-date", &signed.date)
            .header("x-amz-content-sha256", payload_hash)
            .header(http::header::AUTHORIZATION, signed.authorization);
        if let Some(length) = length {
            request = request.header(http::header::CONTENT_LENGTH, length);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        if small {
            request = request.timeout(REQUEST_TIMEOUT);
        }
        request.send().await.map_err(|err| Error::Unavailable(format!("bucket: {err}")))
    }

    /// The host and path a key is at.
    fn address(&self, key: &str) -> (String, String) {
        let key = utf8_percent_encode(key, ENCODE).to_string().replace("%2F", "/");
        if self.config.path_style {
            let bucket = utf8_percent_encode(&self.config.bucket, ENCODE);
            // The bucket itself (for listing) is `/bucket`, its objects `/bucket/key`.
            (self.host.clone(), if key.is_empty() { format!("/{bucket}") } else { format!("/{bucket}/{key}") })
        } else {
            (format!("{}.{}", self.config.bucket, self.host), format!("/{key}"))
        }
    }
}

/// The SHA-256 of nothing, for requests without a body.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Turns an error answer into an error that says what the bucket said.
async fn check(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let text = response.text().await.unwrap_or_default();
    let code = tag(&text, "Code").unwrap_or_default();
    let message = tag(&text, "Message").unwrap_or_default();
    let what = format!("bucket answered {status}: {code} {message}");
    Err(if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        Error::Unavailable(what.trim().to_string())
    } else {
        Error::internal(what.trim().to_string())
    })
}

/// What a request is signed over.
struct Request<'a> {
    method: &'a str,
    host: &'a str,
    /// Already percent-encoded.
    path: &'a str,
    /// Already canonical (sorted and encoded).
    query: &'a str,
    payload_hash: &'a str,
    /// Other headers to sign, lowercase names.
    extra: &'a [(&'a str, &'a str)],
}

struct Signed {
    date: String,
    authorization: String,
}

/// Signs a request with AWS Signature Version 4.
fn sign(request: &Request<'_>, config: &S3Config, now: SystemTime) -> Signed {
    let date = amz_date(now);
    let day = &date[..8];
    let mut headers: Vec<(&str, &str)> =
        vec![("host", request.host), ("x-amz-content-sha256", request.payload_hash), ("x-amz-date", &date)];
    headers.extend_from_slice(request.extra);
    headers.sort();
    let signed_headers = headers.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(";");
    let canonical_headers: String = headers.iter().map(|(name, value)| format!("{name}:{}\n", value.trim())).collect();
    let canonical = format!(
        "{}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{}",
        request.method, request.path, request.query, request.payload_hash
    );
    let scope = format!("{day}/{}/s3/aws4_request", config.region);
    let to_sign = format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));
    let key = [day, config.region.as_str(), "s3", "aws4_request"]
        .iter()
        .fold(format!("AWS4{}", config.secret_access_key).into_bytes(), |key, part| hmac(&key, part.as_bytes()));
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    Signed {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            config.access_key_id
        ),
        date,
    }
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn canonical_query(query: &[(&str, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(name, value)| {
            (utf8_percent_encode(name, ENCODE).to_string(), utf8_percent_encode(value, ENCODE).to_string())
        })
        .collect();
    pairs.sort();
    pairs.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("&")
}

/// `20130524T000000Z`, in UTC.
fn amz_date(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

struct Page {
    objects: Vec<Object>,
    next: Option<String>,
}

/// Reads a ListObjectsV2 answer.
fn parse_list(xml: &str) -> Result<Page> {
    let mut objects = Vec::new();
    for contents in xml.split("<Contents>").skip(1) {
        let contents = contents.split("</Contents>").next().unwrap_or_default();
        let key = tag(contents, "Key").ok_or_else(|| Error::internal("bucket listing has an object without a key"))?;
        let size = tag(contents, "Size").and_then(|size| size.parse().ok()).unwrap_or(0);
        objects.push(Object { key, size });
    }
    let truncated = tag(xml, "IsTruncated").is_some_and(|value| value == "true");
    let next = if truncated {
        Some(
            tag(xml, "NextContinuationToken")
                .ok_or_else(|| Error::internal("bucket listing is cut short without saying where it goes on"))?,
        )
    } else {
        None
    };
    Ok(Page { objects, next })
}

/// The text of the first `<name>…</name>` in `xml`, unescaped.
fn tag(xml: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = start + xml[start..].find(&format!("</{name}>"))?;
    Some(unescape(&xml[start..end]))
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> S3Config {
        S3Config {
            bucket: "examplebucket".into(),
            endpoint: "https://s3.amazonaws.com".into(),
            region: "us-east-1".into(),
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            path_style: false,
        }
    }

    fn may_24_2013() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_369_353_600)
    }

    /// The worked examples in AWS's "Authenticating requests: Using the
    /// Authorization header" guide.
    #[test]
    fn signs_like_aws_examples() {
        let get = sign(
            &Request {
                method: "GET",
                host: "examplebucket.s3.amazonaws.com",
                path: "/test.txt",
                query: "",
                payload_hash: EMPTY_SHA256,
                extra: &[("range", "bytes=0-9")],
            },
            &example(),
            may_24_2013(),
        );
        assert_eq!(get.date, "20130524T000000Z");
        assert_eq!(
            get.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );

        let list = sign(
            &Request {
                method: "GET",
                host: "examplebucket.s3.amazonaws.com",
                path: "/",
                query: &canonical_query(&[("max-keys", "2".into()), ("prefix", "J".into())]),
                payload_hash: EMPTY_SHA256,
                extra: &[],
            },
            &example(),
            may_24_2013(),
        );
        assert!(
            list.authorization.ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"),
            "{}",
            list.authorization
        );
    }

    #[test]
    fn dates_are_utc() {
        assert_eq!(amz_date(UNIX_EPOCH), "19700101T000000Z");
        assert_eq!(amz_date(UNIX_EPOCH + Duration::from_secs(951_782_400 + 3661)), "20000229T010101Z");
        assert_eq!(amz_date(UNIX_EPOCH + Duration::from_secs(1_790_985_599)), "20261002T235959Z");
    }

    #[test]
    fn addresses_keys_both_ways() {
        let virtual_hosted = S3::new(example()).unwrap();
        assert_eq!(
            virtual_hosted.address("servers/01J/a b+c"),
            ("examplebucket.s3.amazonaws.com".into(), "/servers/01J/a%20b%2Bc".into())
        );
        let path_style =
            S3::new(S3Config { path_style: true, endpoint: "http://localhost:9000/".into(), ..example() }).unwrap();
        assert_eq!(path_style.address("node/x"), ("localhost:9000".into(), "/examplebucket/node/x".into()));
        assert_eq!(path_style.address(""), ("localhost:9000".into(), "/examplebucket".into()));
        assert_eq!(virtual_hosted.address("").1, "/");
        assert!(S3::new(S3Config { endpoint: "t3.storageapi.dev".into(), ..example() }).is_err());
    }

    #[test]
    fn reads_listings() {
        let page = parse_list(
            "<?xml version=\"1.0\"?><ListBucketResult><Name>b</Name><IsTruncated>true</IsTruncated>\
             <Contents><Key>node/a&amp;b</Key><Size>12</Size></Contents>\
             <Contents><Key>node/c</Key><LastModified>x</LastModified><Size>0</Size></Contents>\
             <NextContinuationToken>tok/=</NextContinuationToken></ListBucketResult>",
        )
        .unwrap();
        assert_eq!(
            page.objects,
            [Object { key: "node/a&b".into(), size: 12 }, Object { key: "node/c".into(), size: 0 }]
        );
        assert_eq!(page.next.as_deref(), Some("tok/="));
        let last = parse_list("<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>").unwrap();
        assert!(last.objects.is_empty() && last.next.is_none());
    }
}
