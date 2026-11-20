//! Project actions use the owning server and never submit a turn.
use super::*;
use codex_app_server_protocol::ThreadMetadataUpdateParams;
use codex_app_server_protocol::ThreadMetadataUpdateResponse;
use codex_app_server_protocol::ThreadSetNameParams;
use codex_app_server_protocol::ThreadSetNameResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ProjectCreateParams;
use codex_app_server_protocol::ProjectCreateResponse;
use codex_app_server_protocol::ProjectDeleteParams;
use codex_app_server_protocol::ProjectDeleteResponse;
use codex_app_server_protocol::ProjectRoot;

#[derive(Clone)]
pub(super) enum Operation {
    Chat,
    Create { name: String, root: PathBuf },
    Delete,
}

pub(super) enum ActionResult {
    Thread(Thread),
    Projects(Vec<PickerProject>),
}

pub(super) enum Management {
    Create { name: String, root: String, field: usize, pending: bool },
    Delete { project: PickerProject, pending: bool },
}

pub(super) struct Destination {
    pub(super) thread: Option<(ThreadId, String)>,
    pub(super) selected: usize,
    pub(super) pending: bool,
}

pub(super) fn start_params(project: &PickerProject) -> std::io::Result<ThreadStartParams> {
    let root = project
        .roots
        .first()
        .ok_or_else(|| std::io::Error::other("Project has no root directory"))?;
    Ok(ThreadStartParams {
        project_id: Some(project.id.clone()),
        cwd: Some(root.to_string_lossy().into_owned()),
        ephemeral: Some(false),
        ..Default::default()
    })
}

pub(super) fn move_params(
    thread_id: ThreadId,
    project: &PickerProject,
) -> ThreadMetadataUpdateParams {
    ThreadMetadataUpdateParams {
        thread_id: thread_id.to_string(),
        project_id: Some(project.id.clone()),
        git_info: None,
        daybreak_enabled: None,
    }
}

pub(super) async fn execute(
    handle: &AppServerRequestHandle,
    project: PickerProject,
    thread_id: Option<ThreadId>,
    operation: Operation,
) -> std::io::Result<ActionResult> {
    let request_id = RequestId::String(format!("picker-project-action-{}", Uuid::new_v4()));
    match operation {
        Operation::Create { name, root } => {
            let absolute = codex_utils_absolute_path::AbsolutePathBuf::try_from(root)
                .map_err(std::io::Error::other)?;
            handle.request_typed::<ProjectCreateResponse>(ClientRequest::ProjectCreate {
                request_id,
                params: ProjectCreateParams {
                    name,
                    roots: vec![ProjectRoot { path: absolute }],
                    metadata: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                },
            }).await.map_err(std::io::Error::other)?;
            return super::projects::load(handle.clone()).await
                .map(ActionResult::Projects).map_err(std::io::Error::other);
        }
        Operation::Delete => {
            handle.request_typed::<ProjectDeleteResponse>(ClientRequest::ProjectDelete {
                request_id,
                params: ProjectDeleteParams { project_id: project.id },
            }).await.map_err(std::io::Error::other)?;
            return super::projects::load(handle.clone()).await
                .map(ActionResult::Projects).map_err(std::io::Error::other);
        }
        Operation::Chat => {}
    }
    if let Some(thread_id) = thread_id {
        handle
            .request_typed::<ThreadMetadataUpdateResponse>(ClientRequest::ThreadMetadataUpdate {
                request_id,
                params: move_params(thread_id, &project),
            })
            .await
            .map(|response| ActionResult::Thread(response.thread))
            .map_err(std::io::Error::other)
    } else {
        let response = handle
            .request_typed::<ThreadStartResponse>(ClientRequest::ThreadStart {
                request_id,
                params: start_params(&project)?,
            })
            .await
            .map_err(std::io::Error::other)?;
        // Empty threads are staged until a write persists them. The picker owns
        // a temporary connection, so save before returning a resume target.
        handle
            .request_typed::<ThreadSetNameResponse>(ClientRequest::ThreadSetName {
                request_id: RequestId::String(format!("picker-project-save-{}", Uuid::new_v4())),
                params: ThreadSetNameParams {
                    thread_id: response.thread.id.clone(),
                    name: "New chat".into(),
                },
            })
            .await
            .map_err(std::io::Error::other)?;
        Ok(ActionResult::Thread(response.thread))
    }
}

impl PickerState {
    pub(super) fn open_project_create(&mut self) {
        if !matches!(self.action, SessionPickerAction::Resume) { return; }
        let root = self.filter_cwd.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        self.project_management = Some(Management::Create { name: String::new(), root, field: 0, pending: false });
        self.project_destination = Some(Destination { thread: None, selected: 0, pending: false });
        self.request_frame();
    }

    pub(super) fn open_project_delete(&mut self) {
        if !matches!(self.action, SessionPickerAction::Resume) { return; }
        let Some(project) = self.projects.get(self.selected_project).cloned() else { return; };
        self.project_management = Some(Management::Delete { project, pending: false });
        self.project_destination = Some(Destination { thread: None, selected: 0, pending: false });
        self.request_frame();
    }

    pub(super) fn handle_management_key(&mut self, key: KeyEvent) {
        let Some(management) = self.project_management.as_mut() else { return; };
        match management {
            Management::Create { name, root, field, pending } => {
                if *pending { return; }
                match key.code {
                    KeyCode::Esc => { self.project_management = None; self.project_destination = None; }
                    KeyCode::Tab | KeyCode::Down | KeyCode::Up => *field = 1 - *field,
                    KeyCode::Backspace => { if *field == 0 { name.pop(); } else { root.pop(); } }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                        if *field == 0 { name.push(c); } else { root.push(c); }
                    }
                    KeyCode::Enter => {
                        let trimmed = name.trim();
                        let path = PathBuf::from(root.trim());
                        if trimmed.is_empty() || !path.is_absolute() {
                            self.inline_error = Some("Enter a name and an absolute root path".into());
                        } else {
                            *pending = true;
                            (self.picker_loader)(PickerLoadRequest::ProjectAction {
                                backend_id: self.backend_id,
                                project: PickerProject { id: String::new(), name: String::new(), roots: Vec::new() },
                                thread_id: None,
                                operation: Operation::Create { name: trimmed.into(), root: path },
                            });
                        }
                    }
                    _ => {}
                }
            }
            Management::Delete { project, pending } => {
                if *pending { return; }
                match key.code {
                    KeyCode::Esc => { self.project_management = None; self.project_destination = None; }
                    KeyCode::Enter => {
                        *pending = true;
                        (self.picker_loader)(PickerLoadRequest::ProjectAction {
                            backend_id: self.backend_id, project: project.clone(), thread_id: None,
                            operation: Operation::Delete,
                        });
                    }
                    _ => {}
                }
            }
        }
        self.request_frame();
    }

    pub(super) fn open_project_destination(&mut self, moving: bool) {
        if !matches!(self.action, SessionPickerAction::Resume)
            || !matches!(self.archive_state, archive::ArchiveState::Idle)
        {
            return;
        }
        if !self.projects_available || self.projects.is_empty() {
            self.inline_error = Some("No saved projects available".into());
            self.request_frame();
            return;
        }
        let thread = if moving {
            let Some(row) = self.filtered_rows.get(self.selected) else {
                return;
            };
            let Some(id) = row.thread_id else { return };
            if row.backend_id.unwrap_or(self.backend_id) != self.backend_id {
                self.inline_error =
                    Some("Open this chat's backend picker to choose its projects".into());
                self.request_frame();
                return;
            }
            Some((id, row.display_preview().to_string()))
        } else {
            None
        };
        self.project_destination = Some(Destination {
            thread,
            selected: self.selected_project.min(self.projects.len() - 1),
            pending: false,
        });
        self.request_frame();
    }

    pub(super) fn handle_destination_key(&mut self, key: KeyEvent) {
        let Some(destination) = self.project_destination.as_mut() else {
            return;
        };
        if destination.pending {
            return;
        }
        match key.code {
            KeyCode::Esc => self.project_destination = None,
            KeyCode::Up => destination.selected = destination.selected.saturating_sub(1),
            KeyCode::Down => {
                destination.selected = (destination.selected + 1).min(self.projects.len() - 1)
            }
            KeyCode::Enter => {
                destination.pending = true;
                (self.picker_loader)(PickerLoadRequest::ProjectAction {
                    backend_id: self.backend_id,
                    project: self.projects[destination.selected].clone(),
                    thread_id: destination.thread.as_ref().map(|(id, _)| *id),
                    operation: Operation::Chat,
                });
            }
            _ => {}
        }
        self.request_frame();
    }
}

pub(super) fn render(frame: &mut crate::custom_terminal::Frame, area: Rect, state: &PickerState) {
    if let Some(management) = &state.project_management {
        let lines = match management {
            Management::Create { name, root, field, pending } => vec![
                Line::from("Create project").bold(),
                Line::from(format!("{} Name: {}", if *field == 0 { ">" } else { " " }, name)),
                Line::from(format!("{} Root: {}", if *field == 1 { ">" } else { " " }, root)),
                Line::from(if *pending { "Creating..." } else { "Tab switch field | Enter create | Esc cancel" }),
            ],
            Management::Delete { project, pending } => vec![
                Line::from("Delete project?").bold(),
                Line::from(format!("Name: {}", project.name)),
                Line::from(format!("Root: {}", project.roots.first().map(|p| p.display().to_string()).unwrap_or_default())),
                Line::from("Member chats (active and archived) become unassigned."),
                Line::from("Chats, files, and each chat's cwd stay unchanged."),
                Line::from(if *pending { "Deleting..." } else { "Enter confirm delete | Esc cancel" }),
            ],
        };
        frame.render_widget_ref(&ratatui::widgets::Paragraph::new(lines), area);
        return;
    }
    let Some(destination) = &state.project_destination else {
        return;
    };
    let title = match &destination.thread {
        Some((_, name)) => format!("Move chat: {name}"),
        None => "New chat in project".into(),
    };
    let mut lines = vec![
        Line::from(title).bold(),
        Line::from(if destination.pending {
            "Applying project action..."
        } else {
            "Choose destination: Up/Down | Enter apply | Esc cancel"
        }),
    ];
    let visible = usize::from(area.height.saturating_sub(2)).max(1);
    let start = destination.selected.saturating_sub(visible - 1);
    for (index, project) in state.projects.iter().enumerate().skip(start).take(visible) {
        let root = project
            .roots
            .first()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "no root".into());
        let line = Line::from(format!(
            "{} {} — {}",
            if index == destination.selected {
                ">"
            } else {
                " "
            },
            project.name,
            root
        ));
        lines.push(if index == destination.selected {
            line.bold()
        } else {
            line
        });
    }
    frame.render_widget_ref(&ratatui::widgets::Paragraph::new(lines), area);
}
