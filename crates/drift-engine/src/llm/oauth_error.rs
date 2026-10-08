#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error(transparent)]
    Send(#[from] super::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    CallbackIo(#[from] std::io::Error),
    #[error("port {port} is busy: {source}")]
    CallbackPort { port: u16, source: std::io::Error },
    #[error("callback carried a bad state or no code")]
    CallbackState,
    #[error("token request failed ({status}): {text}")]
    TokenResponse { status: reqwest::StatusCode, text: String },
    #[error("token response lacks {0}")]
    MissingTokenField(String),
    #[error("xAI's answer lacks {0}")]
    MissingDeviceField(String),
    #[error("xAI did not start the sign-in ({0})")]
    DeviceStart(reqwest::StatusCode),
    #[error("xAI did not renew the sign-in ({0})")]
    DeviceRefresh(reqwest::StatusCode),
    #[error("xAI refused the sign-in ({status}): {reason}")]
    DeviceRefused {
        status: reqwest::StatusCode,
        reason: String,
    },
    #[error("the sign-in was declined")]
    Declined,
    #[error("the code expired; start the sign-in again")]
    Expired,
    #[error("the code expired before it was approved; start the sign-in again")]
    ExpiredBeforeApproval,
}

#[cfg(test)]
mod tests {
    use super::OAuthError;

    #[test]
    fn provider_errors_keep_the_existing_sign_in_text() {
        let cases = [
            (OAuthError::Declined, "the sign-in was declined"),
            (OAuthError::Expired, "the code expired; start the sign-in again"),
            (
                OAuthError::ExpiredBeforeApproval,
                "the code expired before it was approved; start the sign-in again",
            ),
            (OAuthError::CallbackState, "callback carried a bad state or no code"),
            (
                OAuthError::MissingTokenField("access_token".into()),
                "token response lacks access_token",
            ),
            (
                OAuthError::MissingDeviceField("refresh_token".into()),
                "xAI's answer lacks refresh_token",
            ),
        ];

        for (error, text) in cases {
            assert_eq!(error.to_string(), text);
        }
    }
}
