use super::*;

fn state() -> PickerState {
    let mut state = PickerState::new(
        crate::backend_id::BackendId::new(),
        FrameRequester::test_dummy(),
        Arc::new(|_| {}),
        ProviderFilter::Any,
        true,
        None,
        SessionPickerAction::Resume,
    );
    state.projects_available = true;
    state.projects = vec![
        PickerProject {
            id: "app".into(),
            name: "App".into(),
            roots: vec![PathBuf::from("/work/app")],
        },
        PickerProject {
            id: "docs".into(),
            name: "Docs".into(),
            roots: vec![PathBuf::from("/work/docs")],
        },
    ];
    state
}

fn row(cwd: &str) -> Row {
    let mut row = tests::make_row(cwd, "2026-11-04T00:00:00Z", "Example");
    row.cwd = Some(PathBuf::from(cwd));
    row
}

#[test]
fn project_filter_includes_subdirectories_and_excludes_similar_names() {
    let mut state = state();
    state.filter_mode = SessionFilterMode::Project;
    assert!(state.row_matches_filter(&row("/work/app")));
    assert!(state.row_matches_filter(&row("/work/app/src")));
    assert!(!state.row_matches_filter(&row("/work/app-other")));
    assert!(!state.row_matches_filter(&row("/work/docs")));
}

#[test]
fn non_project_excludes_every_saved_project_root() {
    let mut state = state();
    state.filter_mode = SessionFilterMode::NonProject;
    assert!(!state.row_matches_filter(&row("/work/app/src")));
    assert!(!state.row_matches_filter(&row("/work/docs")));
    assert!(state.row_matches_filter(&row("/work/app-other")));
    assert!(state.row_matches_filter(&row("/home/user")));
    let mut unknown = row("/unknown");
    unknown.cwd = None;
    assert!(!state.row_matches_filter(&unknown));
}

#[test]
fn filter_cycle_and_project_selection_are_keyboard_accessible() {
    let mut state = state();
    state.change_filter_mode(ToolbarValueDirection::Next);
    assert_eq!(state.filter_mode, SessionFilterMode::Project);
    assert_eq!(state.active_cwd_filter(), None);
    state.focus_next_toolbar_control();
    assert_eq!(state.toolbar_focus, ToolbarControl::Project);
    state.change_focused_toolbar_value(ToolbarValueDirection::Next);
    assert_eq!(state.selected_project, 1);
    assert!(state.row_matches_filter(&row("/work/docs/chapter")));
    state.focus_next_toolbar_control();
    assert_eq!(state.toolbar_focus, ToolbarControl::Status);
    state.focus_previous_toolbar_control();
    assert_eq!(state.toolbar_focus, ToolbarControl::Project);
    state.focus_previous_toolbar_control();
    state.change_filter_mode(ToolbarValueDirection::Next);
    assert_eq!(state.filter_mode, SessionFilterMode::NonProject);
    state.change_filter_mode(ToolbarValueDirection::Next);
    assert_eq!(state.filter_mode, SessionFilterMode::All);
}

#[test]
fn unavailable_catalog_skips_project_filters_in_both_directions() {
    let mut state = state();
    state.projects_available = false;
    state.filter_cwd = Some(PathBuf::from("/home/user"));
    state.repo_filter_available = true;
    state.change_filter_mode(ToolbarValueDirection::Previous);
    assert_eq!(state.filter_mode, SessionFilterMode::Repo);
    state.filter_mode = SessionFilterMode::All;
    state.change_filter_mode(ToolbarValueDirection::Next);
    assert_eq!(state.filter_mode, SessionFilterMode::Cwd);
}

#[test]
fn project_toolbar_shows_options_even_without_current_directory_filter() {
    let state = state();
    let text = Line::from(filter_control_spans(&state, false)).to_string();
    assert!(text.contains("Project"));
    assert!(text.contains("Non-project"));
    assert!(!text.contains("Cwd"));
}

#[test]
fn explicit_assignment_overrides_cwd_and_non_project() {
    let mut state = state();
    let mut assigned = row("/work/app");
    assigned.project_id = Some("docs".into());
    state.filter_mode = SessionFilterMode::Project;
    assert!(!state.row_matches_filter(&assigned));
    state.selected_project = 1;
    assert!(state.row_matches_filter(&assigned));
    assigned.cwd = None;
    assert!(state.row_matches_filter(&assigned));
    state.filter_mode = SessionFilterMode::NonProject;
    assert!(!state.row_matches_filter(&assigned));
    assigned.cwd = Some(PathBuf::from("/outside"));
    assigned.project_id = Some("unknown-project".into());
    assert!(!state.row_matches_filter(&assigned));
    state.filter_mode = SessionFilterMode::Project;
    assert!(!state.row_matches_filter(&assigned));
}

#[test]
fn project_action_params_use_exact_protocol_fields() {
    let state = state();
    let project = &state.projects[0];
    let start = project_actions::start_params(project).unwrap();
    let json = serde_json::to_value(start).unwrap();
    assert_eq!(json["projectId"], "app");
    assert_eq!(json["cwd"], "/work/app");
    assert_eq!(json["ephemeral"], false);
    assert!(json.get("input").is_none());
    let id = ThreadId::new();
    let update = serde_json::to_value(project_actions::move_params(id, project)).unwrap();
    assert_eq!(update["threadId"], id.to_string());
    assert_eq!(update["projectId"], "app");
    assert!(update["gitInfo"].is_null());
    let mut no_root = project.clone();
    no_root.roots.clear();
    assert!(project_actions::start_params(&no_root).is_err());
}

#[tokio::test]
async fn destination_requires_accept_cancels_and_submits_only_once() {
    let mut state = state();
    let id = ThreadId::new();
    let mut selected = row("/work/app");
    selected.thread_id = Some(id);
    state.filtered_rows = vec![selected];
    let (tx, mut rx) = mpsc::unbounded_channel();
    state.picker_loader = Arc::new(move |request| {
        tx.send(request).unwrap();
    });
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    state.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)).await.unwrap();
    assert!(rx.try_recv().is_err());
    state.handle_key(key(KeyCode::Down)).await.unwrap();
    assert_eq!(state.project_destination.as_ref().unwrap().selected, 1);
    assert!(rx.try_recv().is_err());
    state.handle_key(key(KeyCode::Esc)).await.unwrap();
    assert!(state.project_destination.is_none());
    assert!(rx.try_recv().is_err());
    state.handle_key(key(KeyCode::F(3))).await.unwrap();
    state.handle_key(key(KeyCode::Down)).await.unwrap();
    state.handle_key(key(KeyCode::Enter)).await.unwrap();
    assert!(
        matches!(rx.try_recv().unwrap(), PickerLoadRequest::ProjectAction { project, thread_id: Some(thread), .. } if project.id == "docs" && thread == id)
    );
    state.handle_key(key(KeyCode::Enter)).await.unwrap();
    state.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT)).await.unwrap();
    assert!(rx.try_recv().is_err());
    state
        .handle_background_event(BackgroundEvent::ProjectAction {
            created: false,
            result: Err(std::io::Error::other("test rejection")),
        })
        .await
        .unwrap();
    assert!(state.project_destination.is_none());
    assert!(
        state
            .inline_error
            .as_deref()
            .unwrap()
            .contains("test rejection")
    );
    state.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT)).await.unwrap();
    state.handle_key(key(KeyCode::Enter)).await.unwrap();
    assert!(
        matches!(rx.try_recv().unwrap(), PickerLoadRequest::ProjectAction { project, thread_id: None, .. } if project.id == "app")
    );
}

#[tokio::test]
async fn project_actions_reject_cross_backend_and_missing_projects() {
    let mut state = state();
    let mut selected = row("/work/app");
    selected.thread_id = Some(ThreadId::new());
    selected.backend_id = Some(crate::backend_id::BackendId::new());
    state.filtered_rows = vec![selected];
    state.open_project_destination(true);
    assert!(state.project_destination.is_none());
    assert!(state.inline_error.is_some());
    state.projects.clear();
    state.open_project_destination(false);
    assert!(state.project_destination.is_none());
}

#[test]
fn project_action_hints_and_destination_render() {
    let mut state = state();
    let hints = footer_hint_lines(&state, 120)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(hints.contains("Alt+Z"));
    assert!(hints.contains("Alt+X"));
    assert!(hints.contains("Alt+N"));
    assert!(hints.contains("Alt+D"));
    state.open_project_destination(false);
    let backend = crate::test_backend::VT100Backend::new(100, 15);
    let mut terminal = crate::custom_terminal::Terminal::with_options(backend).unwrap();
    terminal.set_viewport_area(Rect::new(0, 0, 100, 15));
    {
        let mut frame = terminal.get_frame();
        let area = frame.area();
        project_actions::render(&mut frame, area, &state);
    }
    terminal.flush().unwrap();
    let text = terminal.backend().to_string();
    assert!(text.contains("New chat in project"));
    assert!(text.contains("Enter apply"));
    assert!(text.contains("App"));
    assert!(text.contains("/work/docs"));
}

#[tokio::test]
async fn project_management_requires_explicit_enter_and_uses_entered_root() {
    let mut state = state();
    state.filter_cwd = Some(PathBuf::from("C:\\work"));
    let (tx, mut rx) = mpsc::unbounded_channel();
    state.picker_loader = Arc::new(move |request| { tx.send(request).unwrap(); });
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT)).await.unwrap();
    assert!(rx.try_recv().is_err());
    state.handle_key(key(KeyCode::Esc)).await.unwrap();
    assert!(rx.try_recv().is_err());
    state.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT)).await.unwrap();
    for c in "Example".chars() { state.handle_key(key(KeyCode::Char(c))).await.unwrap(); }
    state.handle_key(key(KeyCode::Tab)).await.unwrap();
    state.handle_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(rx.try_recv().unwrap(), PickerLoadRequest::ProjectAction { operation: project_actions::Operation::Create { name, root }, .. } if name == "Example" && root == PathBuf::from("C:\\work")));
}
