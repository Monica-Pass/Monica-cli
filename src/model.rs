use clap::ValueEnum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::error::{GatewayError, Result};

pub const CONNECTION_CATALOG_TOOL: &str = "monica_list_connections";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Github,
    Gitlab,
}

impl Provider {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
        }
    }

    pub fn default_api_base(self) -> &'static str {
        match self {
            Self::Github => "https://api.github.com/",
            Self::Gitlab => "https://gitlab.com/api/v4/",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    ListIssues,
    GetIssue,
    CreateIssue,
    ApiRead,
    ApiWrite,
}

impl Operation {
    pub const ALL: [Self; 5] = [
        Self::ListIssues,
        Self::GetIssue,
        Self::CreateIssue,
        Self::ApiRead,
        Self::ApiWrite,
    ];
    pub fn is_write(self) -> bool {
        matches!(self, Self::CreateIssue | Self::ApiWrite)
    }
    pub fn is_api(self) -> bool {
        matches!(self, Self::ApiRead | Self::ApiWrite)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::ListIssues => "list_issues",
            Self::GetIssue => "get_issue",
            Self::CreateIssue => "create_issue",
            Self::ApiRead => "api_read",
            Self::ApiWrite => "api_write",
        }
    }

    pub fn tool_name(self, provider: Provider) -> String {
        format!("{}_{}", provider.prefix(), self.name())
    }

    pub fn from_tool(name: &str, provider: Provider) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.tool_name(provider) == name)
            .ok_or(GatewayError::PermissionDenied)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub tool: String,
    pub arguments: Value,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListIssuesArgs {
    /// Exact repository path authorized by the human, such as org/repository.
    #[schemars(length(min = 3, max = 512))]
    pub repository: String,
    #[serde(default = "default_page")]
    #[schemars(range(min = 1, max = 10000))]
    pub page: u32,
    #[serde(default = "default_page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: u32,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    20
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetIssueArgs {
    #[schemars(length(min = 3, max = 512))]
    pub repository: String,
    /// GitHub issue number or GitLab issue IID.
    #[schemars(range(min = 1))]
    pub number: u64,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateIssueArgs {
    #[schemars(length(min = 3, max = 512))]
    pub repository: String,
    #[schemars(length(min = 1, max = 256))]
    pub title: String,
    #[serde(default)]
    #[schemars(length(max = 32768))]
    pub body: String,
    /// A new UUID for this intended write. Reuse it when checking a previous attempt.
    pub request_id: String,
}

#[derive(Serialize)]
#[serde(tag = "operation", content = "arguments", rename_all = "snake_case")]
pub enum Arguments {
    List(ListIssuesArgs),
    Get(GetIssueArgs),
    Create(CreateIssueArgs),
    Api(crate::service_api::ApiArgs),
}

impl Arguments {
    pub fn parse(operation: Operation, value: Value, provider: Provider) -> Result<Self> {
        if operation.is_api() {
            return Ok(Self::Api(crate::service_api::ApiArgs::parse(
                operation, value,
            )?));
        }
        let parsed = match operation {
            Operation::ListIssues => {
                let args: ListIssuesArgs =
                    serde_json::from_value(value).map_err(|_| GatewayError::InvalidRequest)?;
                if args.page == 0 || args.page > 10_000 || !(1..=100).contains(&args.per_page) {
                    return Err(GatewayError::InvalidRequest);
                }
                Self::List(args)
            }
            Operation::GetIssue => {
                let args: GetIssueArgs =
                    serde_json::from_value(value).map_err(|_| GatewayError::InvalidRequest)?;
                if args.number == 0 || args.number > i64::MAX as u64 {
                    return Err(GatewayError::InvalidRequest);
                }
                Self::Get(args)
            }
            Operation::CreateIssue => {
                let mut args: CreateIssueArgs =
                    serde_json::from_value(value).map_err(|_| GatewayError::InvalidRequest)?;
                if args.title.trim().is_empty()
                    || args.title.len() > 256
                    || args.body.len() > 32 * 1024
                    || args.title.chars().any(char::is_control)
                    || uuid::Uuid::parse_str(&args.request_id).is_err()
                {
                    return Err(GatewayError::InvalidRequest);
                }
                args.request_id = uuid::Uuid::parse_str(&args.request_id)
                    .map_err(|_| GatewayError::InvalidRequest)?
                    .to_string();
                Self::Create(args)
            }
            Operation::ApiRead | Operation::ApiWrite => unreachable!(),
        };
        validate_repository(parsed.repository(), provider)?;
        Ok(parsed)
    }

    pub fn repository(&self) -> &str {
        match self {
            Self::List(args) => &args.repository,
            Self::Get(args) => &args.repository,
            Self::Create(args) => &args.repository,
            Self::Api(args) => &args.repository,
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Create(args) => Some(&args.request_id),
            Self::Api(args) => args.request_id.as_deref(),
            _ => None,
        }
    }
}

pub fn validate_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(GatewayError::InvalidConfig);
    }
    Ok(())
}

pub const MAX_NOTE_BYTES: usize = 1024;

/// Notes are public data, never terminal commands or policy instructions.
pub fn validate_note(value: &str) -> Result<()> {
    if value.len() > MAX_NOTE_BYTES
        || value.chars().any(|ch| {
            ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
    {
        return Err(GatewayError::InvalidNote);
    }
    Ok(())
}

pub fn validate_repository(value: &str, provider: Provider) -> Result<()> {
    let segments: Vec<_> = value.split('/').collect();
    if value.len() > 512
        || segments.len() < 2
        || segments.len() > 12
        || (provider == Provider::Github && segments.len() != 2)
        || segments.iter().any(|segment| {
            segment.is_empty()
                || segment.len() > 100
                || *segment == "."
                || *segment == ".."
                || !segment
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        })
    {
        return Err(GatewayError::InvalidRequest);
    }
    Ok(())
}

pub fn validate_api_base(value: &str, provider: Provider) -> Result<Url> {
    if value.len() > 2048 {
        return Err(GatewayError::InvalidConfig);
    }
    let url = Url::parse(value).map_err(|_| GatewayError::InvalidConfig)?;
    let valid_path = match provider {
        Provider::Github => matches!(url.path(), "/" | "/api/v3/"),
        Provider::Gitlab => url.path() == "/api/v4/",
    };
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !valid_path
    {
        return Err(GatewayError::InvalidConfig);
    }
    Ok(url)
}

/// Service-wide API grants are explicit and cannot masquerade as repository grants.
pub fn validate_grant_scope(
    repositories: &[String],
    operations: &[Operation],
    provider: Provider,
) -> Result<()> {
    if operations.iter().any(|op| op.is_api()) {
        if repositories != ["*"] || operations.iter().any(|op| !op.is_api()) {
            return Err(GatewayError::InvalidRequest);
        }
    } else {
        for repository in repositories {
            validate_repository(repository, provider)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_rejects_paths_headers_urls_and_unbounded_arguments() {
        for repository in [
            "org/../secret",
            "org/%2e%2e",
            "org/repo?x=1",
            "org//repo",
            "org/repo\\other",
            "org/repo\n",
        ] {
            assert!(validate_repository(repository, Provider::Gitlab).is_err());
        }
        assert!(validate_repository("org/subgroup/repo", Provider::Gitlab).is_ok());
        assert!(validate_repository("org/subgroup/repo", Provider::Github).is_err());
        for arguments in [
            json!({"repository":"org/repo", "headers":{"Authorization":"x"}}),
            json!({"repository":"org/repo", "url":"https://other.example"}),
            json!({"repository":"org/repo", "per_page":101}),
            json!({"repository":"org/repo", "page":0}),
        ] {
            assert!(Arguments::parse(Operation::ListIssues, arguments, Provider::Github).is_err());
        }
        for base in [
            "http://api.github.com/",
            "https://user:password@api.github.com/",
            "https://api.github.com/?url=x",
            "https://api.github.com/other/",
        ] {
            assert!(validate_api_base(base, Provider::Github).is_err());
        }
    }
}
