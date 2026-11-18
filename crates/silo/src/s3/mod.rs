// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Path-style S3-compatible routes on the silo router.
//!
//! ```text
//! PUT                         /{bucket}          CreateBucket
//! HEAD                        /{bucket}          HeadBucket
//! DELETE                      /{bucket}          DeleteBucket
//! GET                         /{bucket}?list-type=2&prefix=&delimiter=&continuation-token=&start-after=&max-keys=
//! GET                         /{bucket}?uploads&prefix=&delimiter=&key-marker=&upload-id-marker=&max-uploads=
//!                                     ListMultipartUploads
//! GET                         /{bucket}?versions&prefix=&max-keys=&key-marker=&version-id-marker=
//!                                     ListObjectVersions
//! GET|PUT                     /{bucket}?versioning   Get/PutBucketVersioning
//! POST                        /{bucket}?delete (XML body)   DeleteObjects
//! PUT|GET|HEAD|DELETE|POST    /{bucket}/{*key}
//!
//! Multipart (query on the object path):
//! POST   ?uploads                         CreateMultipartUpload
//! PUT    ?partNumber=N&uploadId=X         UploadPart
//! PUT    ?partNumber=N&uploadId=X         UploadPartCopy (`x-amz-copy-source`)
//! PUT    (no multipart query)             CopyObject (`x-amz-copy-source`)
//! POST   ?uploadId=X  (XML body)          CompleteMultipartUpload
//! DELETE ?uploadId=X                      AbortMultipartUpload
//! GET    ?uploadId=X                      ListParts
//! ```
//!
//! Auth: gate bearer `Caller` (request extensions) or AWS Signature
//! Version 4 ([`crate::sigv4`]) via keep access keys. Header SigV4,
//! presigned GET/PUT, and aws-chunked streaming PutObject/UploadPart
//! (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`,
//! `STREAMING-UNSIGNED-PAYLOAD-TRAILER`,
//! `STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`) are supported.
//! **UploadPartCopy** and **CopyObject** (`x-amz-copy-source`) stream from a
//! source object in a bucket visible to the caller. A foreign-project or
//! nonexistent copy-source bucket returns `AccessDenied` (existence is not
//! leaked); a missing key in a visible bucket is still `NoSuchKey`.
//! Copy-source `versionId` is `NotImplemented`. `x-amz-copy-source-range`
//! is only valid on UploadPartCopy. Single-PUT ETag is the content MD5
//! hex (quoted); completed multipart ETag is `{md5-of-part-md5s}-{n}`
//! (quoted).
//!
//! Staging dirs under `.uploads/` are removed on abort and complete, and
//! by the [`crate::gc`] sweep once older than `multipart_ttl_secs`.
//! ListObjectsV2 and ListMultipartUploads support `delimiter` /
//! `CommonPrefixes` (any non-empty delimiter; shared helpers in
//! [`crate::delimiter`]) and `encoding-type=url` (percent-encode key-name
//! fields; echo `<EncodingType>url</EncodingType>`). Other non-empty
//! encoding-type values are `InvalidArgument`. `fetch-owner` is ignored.
//! Initiator/Owner are not reported. ListObjectsV2 continuation tokens are
//! opaque URL-safe base64. PutObject, CreateMultipartUpload, and CopyObject
//! destination keys reject XML-unsafe characters (C0 controls, U+007F,
//! U+FFFE, U+FFFF) after bucket visibility; GET/HEAD/DELETE leave existing
//! keys readable.
//!
//! **GetObject** answers a `Range` header of `bytes=a-b`, `bytes=a-` or
//! `bytes=-n` with 206, `Content-Range`, and the requested bytes — seeked to
//! and length-limited on disk, never buffered whole. A first byte at or past
//! the end is `InvalidRange` with 416 and `Content-Range: bytes */size`. A
//! multi-range or unreadable header, and `first > last`, are ignored and the
//! whole object is served with 200, as is a range naming all of it. GET and
//! HEAD both answer `Accept-Ranges: bytes`.
//!
//! **DeleteObjects** (`POST /{bucket}?delete`, XML body) takes up to 1000
//! keys and honours the declared body hash the way CompleteMultipartUpload
//! does. A key that is not there counts as deleted; `Quiet`, on the request or
//! on one object, drops that `<Deleted>` entry. Anything else that fails
//! becomes one `<Error>` naming the key, and the answer is still 200: a batch
//! does not fail as a whole over one key.
//!
//! **Versioning.** A `versionId` on the wire is the record's generation
//! number as a decimal string — all the store keeps. `PUT`/`GET
//! /{bucket}?versioning` sets and reads [`BucketSpec::versioning`]
//! ([`crate::s3_bucket_versioning`]); `GET /{bucket}?versions` is
//! ListObjectVersions over every generation ([`crate::s3_list_versions`]).
//! PutObject on a versioned bucket answers `x-amz-version-id`, and
//! GetObject/HeadObject accept `?versionId=` — answering that generation with
//! `x-amz-version-id`, or `NoSuchVersion` with 404 when it is not there.
//! `DELETE ?versionId=` removes exactly that generation, promoting the newest
//! survivor to current when the current one goes. A plain DeleteObject still
//! removes the whole key: the store has no delete marker, so none is listed
//! and none is created.

mod apply_object_headers;
mod bucket_exists;
mod bucket_query;
mod caller;
mod check_content_md5;
mod content_length;
mod copy_object_inner;
mod decode_continuation_token;
mod decoded_body_stream;
mod delete_marker;
mod delete_object;
mod delete_object_inner;
mod delete_objects;
mod delete_objects_inner;
mod delete_objects_xml;
mod deleted_key;
mod encode_continuation_token;
#[cfg(test)]
mod encoding_tests;
mod ensure_body_hash;
mod error_xml;
mod etag_value;
mod failed_key;
mod get_object;
mod get_object_inner;
mod head_bucket;
mod head_bucket_inner;
mod head_object;
mod header_str;
mod key_not_there;
mod list_bucket_page;
mod list_bucket_xml;
mod list_or_bucket;
mod list_or_bucket_inner;
mod list_parts_inner;
mod list_query;
mod list_uploads_inner;
mod list_uploads_page;
mod list_uploads_xml;
mod maybe_decode_chunked;
mod metadata_directive;
mod object_query;
mod parse_encoding_type;
mod parse_limit;
mod parse_max_keys;
mod post_object;
mod post_object_inner;
mod put_object;
mod put_object_inner;
mod s3_error;
mod s3_max_keys;
mod s3_max_uploads;
mod s3_method_not_allowed;
mod s3_url_encode;
mod set_delete_marker;
mod set_version_id;
mod upload_part_copy_inner;
mod version_id;
mod with_object_read;
mod xml_escape;
mod xml_list_value;
pub(crate) use bucket_exists::bucket_exists;
pub use bucket_query::BucketQuery;
pub(crate) use caller::caller;
pub use delete_object::delete_object;
pub use delete_objects::delete_objects;
pub(crate) use ensure_body_hash::ensure_body_hash;
pub use error_xml::error_xml;
pub use get_object::get_object;
pub use head_bucket::head_bucket;
pub use head_object::head_object;
pub use list_or_bucket::list_or_bucket;
pub use list_query::ListQuery;
pub use object_query::ObjectQuery;
pub(crate) use parse_encoding_type::parse_encoding_type;
pub(crate) use parse_max_keys::parse_max_keys;
pub use post_object::post_object;
pub use put_object::put_object;
pub(crate) use s3_error::s3_error;
pub(crate) use xml_escape::xml_escape;
pub(crate) use xml_list_value::xml_list_value;
