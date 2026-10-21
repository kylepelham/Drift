//! A multipart form, built by hand so the HTTP client needs no extra feature for a few small bodies.

pub(crate) struct Form {
    boundary: String,
    body: Vec<u8>,
}

impl Form {
    pub(crate) fn new() -> Self {
        Self {
            boundary: format!("drift-{}", crate::random_hex(12)),
            body: Vec::new(),
        }
    }

    fn head(&mut self, disposition: &str, mime: Option<&str>) {
        let kind = mime.map(|mime| format!("Content-Type: {mime}\r\n")).unwrap_or_default();
        let head = format!(
            "--{}\r\nContent-Disposition: form-data; {disposition}\r\n{kind}\r\n",
            self.boundary
        );
        self.body.extend_from_slice(head.as_bytes());
    }

    pub(crate) fn text(mut self, name: &str, value: &str) -> Self {
        self.head(&format!("name=\"{name}\""), None);
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// One field per value, named alike; none when there are none.
    pub(crate) fn texts<'a>(self, name: &str, values: impl IntoIterator<Item = &'a String>) -> Self {
        values.into_iter().fold(self, |form, value| form.text(name, value))
    }

    pub(crate) fn file(mut self, name: &str, filename: &str, mime: &str, bytes: &[u8]) -> Self {
        self.head(&format!("name=\"{name}\"; filename=\"{filename}\""), Some(mime));
        self.body.extend_from_slice(bytes);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    pub(crate) fn attach(mut self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        request
            .header(
                "content-type",
                format!("multipart/form-data; boundary={}", self.boundary),
            )
            .body(self.body)
    }
}
