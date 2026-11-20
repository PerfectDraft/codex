//! Read the server's saved projects; membership uses directory boundaries, not string prefixes.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ProjectListParams;
use codex_app_server_protocol::ProjectListResponse;
use codex_app_server_protocol::RequestId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PickerProject {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) roots: Vec<PathBuf>,
}

impl PickerProject {
    pub(super) fn contains(&self, cwd: &Path) -> bool {
        self.roots.iter().any(|root| contains_path(root, cwd))
    }
}

fn contains_path(root: &Path, cwd: &Path) -> bool {
    // Interpret the backend's path syntax without asking the host filesystem.
    // In particular, a backslash is a normal filename character on remote Unix.
    fn normalized(path: &Path, windows: bool) -> Option<Vec<String>> {
        let raw = path.to_string_lossy();
        let text = if windows {
            let text = raw.replace('\\', "/").to_lowercase();
            if let Some(unc) = text.strip_prefix("//?/unc/") {
                format!("//{unc}")
            } else {
                text.strip_prefix("//?/").unwrap_or(&text).to_string()
            }
        } else {
            raw.into_owned()
        };
        let mut components = text.split('/').filter(|part| !part.is_empty());
        let prefix = if windows {
            if text.starts_with("//") {
                format!("//{}/{}", components.next()?, components.next()?)
            } else {
                if text.as_bytes().get(1) != Some(&b':') || text.as_bytes().get(2) != Some(&b'/') {
                    return None;
                }
                components.next()?.to_string()
            }
        } else {
            if !text.starts_with('/') {
                return None;
            }
            "/".to_string()
        };
        let mut result = vec![prefix];
        for component in components {
            match component {
                "." => {}
                ".." if result.len() > 1 => {
                    result.pop();
                }
                ".." => {}
                component => result.push(component.to_string()),
            }
        }
        Some(result)
    }
    let root_text = root.to_string_lossy();
    let windows = root_text.as_bytes().get(1) == Some(&b':') || root_text.starts_with("\\\\");
    normalized(root, windows)
        .zip(normalized(cwd, windows))
        .is_some_and(|(root, cwd)| cwd.starts_with(&root))
}

pub(super) async fn load(handle: AppServerRequestHandle) -> Result<Vec<PickerProject>, String> {
    let listing = async {
        let mut projects = Vec::new();
        let mut cursor = None;
        loop {
            let response = handle
                .request_typed::<ProjectListResponse>(ClientRequest::ProjectList {
                    request_id: RequestId::String(format!(
                        "picker-projects-{}",
                        uuid::Uuid::new_v4()
                    )),
                    params: ProjectListParams {
                        cursor: cursor.clone(),
                        limit: Some(100),
                        sort_key: None,
                        sort_direction: None,
                    },
                })
                .await
                .map_err(|error| error.to_string())?;
            for project in response.data {
                let project = PickerProject {
                    id: project.id,
                    name: project.name,
                    roots: project
                        .roots
                        .into_iter()
                        .map(|root| root.path.to_path_buf())
                        .collect(),
                };
                if !projects.contains(&project) {
                    projects.push(project);
                }
            }
            match response.next_cursor {
                Some(next) if cursor.as_ref() != Some(&next) => cursor = Some(next),
                Some(_) => return Err("Project listing returned a repeated cursor".to_string()),
                None => return Ok(projects),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(3), listing)
        .await
        .map_err(|_| "Project listing timed out".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_includes_children_but_not_siblings() {
        let project = PickerProject {
            id: "example".into(),
            name: "Example".to_string(),
            roots: vec![PathBuf::from("/work/app"), PathBuf::from("/work/docs")],
        };
        assert!(project.contains(Path::new("/work/app")));
        assert!(project.contains(Path::new("/work/app/src")));
        assert!(project.contains(Path::new("/work/docs/chapter")));
        assert!(!project.contains(Path::new("/work/app-other")));
        assert!(!project.contains(Path::new("/work")));
        assert!(!project.contains(Path::new("/work/app/../outside")));
        assert!(!project.contains(Path::new("/work/App/src")));
        assert!(!project.contains(Path::new("/work/app\\other")));
        assert!(project.contains(Path::new("/work/app/sub\\dir")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_membership_ignores_case_and_separator_style() {
        assert!(contains_path(
            Path::new("D:\\Work\\App"),
            Path::new("d:/work/app/src")
        ));
        assert!(contains_path(
            Path::new("D:\\Work\\App"),
            Path::new("\\\\?\\D:\\Work\\App\\src")
        ));
        assert!(!contains_path(
            Path::new("D:\\Work\\App"),
            Path::new("D:\\Work\\App2")
        ));
        assert!(contains_path(
            Path::new("\\\\server\\share\\App"),
            Path::new("\\\\?\\UNC\\SERVER\\share\\app\\src")
        ));
    }
}
