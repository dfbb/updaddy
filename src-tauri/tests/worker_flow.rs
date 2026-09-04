use updaddy_lib::commands::{submit_update_command, AppState, PackageId};
use updaddy_lib::core::Ecosystem;

#[test]
fn update_command_returns_task_id_without_waiting_for_worker_completion() {
    let app = AppState::test_with_blocked_worker();
    let task_id = submit_update_command(&app, PackageId::new(Ecosystem::Npm, "eslint")).unwrap();
    assert_ne!(task_id, uuid::Uuid::nil());
    assert!(app.worker_events().is_empty());
}
