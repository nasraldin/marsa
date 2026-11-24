//! Exercise every protocol operation and the durable, private grant lifecycle.

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

use marsa_core::{paths::MarsaPaths, settings::OperateApproval};

use super::*;

const READ: &[AgentOperation] = &[
    AgentOperation::EngineStatus,
    AgentOperation::ListContainers,
    AgentOperation::InspectContainer,
    AgentOperation::ContainerLogs,
    AgentOperation::ContainerStats,
    AgentOperation::ListImages,
    AgentOperation::InspectImage,
    AgentOperation::ListVolumes,
    AgentOperation::ListNetworks,
    AgentOperation::ComposeList,
    AgentOperation::ComposePs,
    AgentOperation::ComposeLogs,
    AgentOperation::DiskUsage,
    AgentOperation::CleanupPreview,
];
const OPERATE: &[AgentOperation] = &[
    AgentOperation::StartContainer,
    AgentOperation::StopContainer,
    AgentOperation::RestartContainer,
    AgentOperation::RunContainer,
    AgentOperation::ExecInContainer,
    AgentOperation::PullImage,
    AgentOperation::ComposeUp,
    AgentOperation::ComposeDown,
    AgentOperation::ComposeRestart,
];
const DESTROY: &[AgentOperation] = &[
    AgentOperation::RemoveContainer,
    AgentOperation::RemoveImage,
    AgentOperation::RemoveNetwork,
    AgentOperation::RemoveVolume,
    AgentOperation::ComposeDownVolumes,
    AgentOperation::Prune,
];

#[test]
fn every_tool_is_classified_for_each_grant_and_operate_setting() {
    // Compare membership to the actual protocol enumeration so a newly added tool
    // cannot silently miss the table.
    let last = AgentOperation::AddShare as i32;
    let project = tempfile::tempdir().unwrap();
    let scope = GrantScope::resolve("client", project.path()).unwrap();
    assert_eq!(
        READ.len() + OPERATE.len() + DESTROY.len() + 1,
        last as usize
    );
    for value in 0..=last {
        let operation = AgentOperation::try_from(value).unwrap();
        let expected_tier = [READ, OPERATE, DESTROY, &[AgentOperation::AddShare]]
            .iter()
            .position(|group| group.contains(&operation))
            .map(|index| PermissionTier::try_from(index as i32 + 1).unwrap());
        assert_eq!(tier(operation), expected_tier);
        for has_grant in [false, true] {
            for operate_approval in [
                OperateApproval::PerProject,
                OperateApproval::AlwaysAsk,
                OperateApproval::Deny,
            ] {
                let settings = AgentSettings {
                    operate_approval,
                    advanced_access: false,
                };
                let mut grants = Grants::default();
                if has_grant && expected_tier == Some(PermissionTier::Operate) {
                    grants
                        .record_approval(
                            scope.clone(),
                            operation,
                            Effect::Ordinary,
                            &AgentSettings::default(),
                            UNIX_EPOCH,
                        )
                        .unwrap();
                }
                let decision = match expected_tier {
                    None => Decision::Refuse(Refusal::UnknownOperation),
                    Some(PermissionTier::Read) => Decision::Allow,
                    Some(PermissionTier::Operate) if operate_approval == OperateApproval::Deny => {
                        Decision::Refuse(Refusal::OperateDisabled)
                    }
                    Some(PermissionTier::Operate)
                        if operate_approval == OperateApproval::PerProject && has_grant =>
                    {
                        Decision::Allow
                    }
                    Some(tier) => Decision::Approve {
                        tier,
                        can_grant_for_project: tier == PermissionTier::Operate
                            && operate_approval == OperateApproval::PerProject,
                    },
                };
                assert_eq!(
                    grants
                        .evaluate(&scope, operation, Effect::Ordinary, &settings, UNIX_EPOCH)
                        .unwrap(),
                    decision,
                    "{operation:?}, {settings:?}, grant={has_grant}"
                );
            }
        }
    }
}

#[test]
fn host_mount_and_privilege_effects_never_reuse_or_create_grants() {
    let scratch = tempfile::tempdir().unwrap();
    let scope = GrantScope::resolve("trusted-client", scratch.path()).unwrap();
    let mut grants = Grants::default();
    let default = AgentSettings::default();
    let advanced = AgentSettings {
        advanced_access: true,
        ..default.clone()
    };
    for operation in [AgentOperation::RunContainer, AgentOperation::ComposeUp] {
        grants
            .record_approval(
                scope.clone(),
                operation,
                Effect::Ordinary,
                &default,
                UNIX_EPOCH,
            )
            .unwrap();
        for access in [
            ContainerAccess {
                privileged: true,
                ..ContainerAccess::default()
            },
            ContainerAccess {
                host_network: true,
                ..ContainerAccess::default()
            },
            ContainerAccess {
                host_pid: true,
                ..ContainerAccess::default()
            },
            ContainerAccess {
                docker_socket: true,
                ..ContainerAccess::default()
            },
        ] {
            assert_eq!(
                grants
                    .evaluate(&scope, operation, access.effect(), &default, UNIX_EPOCH)
                    .unwrap(),
                Decision::Refuse(Refusal::AdvancedDisabled)
            );
            assert_eq!(
                grants
                    .evaluate(&scope, operation, access.effect(), &advanced, UNIX_EPOCH)
                    .unwrap(),
                Decision::Approve {
                    tier: PermissionTier::HostAffecting,
                    can_grant_for_project: false
                }
            );
        }
        for effect in [Effect::Advanced, Effect::HostAffecting, Effect::Destroy] {
            assert!(matches!(
                grants.record_approval(scope.clone(), operation, effect, &advanced, UNIX_EPOCH),
                Err(GrantError::Ineligible)
            ));
        }
    }
    for operation in DESTROY.iter().copied().chain([AgentOperation::AddShare]) {
        assert!(matches!(
            grants.record_approval(
                scope.clone(),
                operation,
                Effect::Ordinary,
                &advanced,
                UNIX_EPOCH
            ),
            Err(GrantError::Ineligible)
        ));
    }
}

#[test]
fn permissions_expire_after_disuse_and_are_bound_to_client_project_and_action() {
    let scratch = tempfile::tempdir().unwrap();
    let first = scratch.path().join("first");
    let second = scratch.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let scope = GrantScope::resolve("client-a", &first).unwrap();
    let other_client = GrantScope::resolve("client-b", &first).unwrap();
    let other_project = GrantScope::resolve("client-a", &second).unwrap();
    let policy = AgentSettings::default();
    let day = Duration::from_secs(24 * 60 * 60);
    let operation = AgentOperation::RunContainer;
    let mut grants = Grants::default();
    let granted = grants
        .record_approval(
            scope.clone(),
            operation,
            Effect::Ordinary,
            &policy,
            UNIX_EPOCH,
        )
        .unwrap();
    for (scope, operation) in [
        (&other_project, operation),
        (&other_client, operation),
        (&scope, AgentOperation::ExecInContainer),
    ] {
        assert!(matches!(
            grants.evaluate(scope, operation, Effect::Ordinary, &policy, UNIX_EPOCH),
            Ok(Decision::Approve { .. })
        ));
    }
    assert_eq!(
        grants
            .evaluate(
                &scope,
                operation,
                Effect::Ordinary,
                &policy,
                UNIX_EPOCH + day * 29
            )
            .unwrap(),
        Decision::Allow
    );
    let renewed = grants
        .active(UNIX_EPOCH + day * 58)
        .unwrap()
        .next()
        .unwrap()
        .to_wire();
    assert_eq!(renewed.id, granted.id);
    assert!(renewed.expires_at_unix_millis > granted.expires_at_unix_millis);
    assert_eq!(grants.active(UNIX_EPOCH + day * 59).unwrap().count(), 0);

    fs::rename(&first, scratch.path().join("moved-project")).unwrap();
    fs::create_dir(&first).unwrap();
    let replacement = GrantScope::resolve("client-a", &first).unwrap();
    assert_ne!(scope, replacement);
    assert!(matches!(
        grants.evaluate(
            &replacement,
            operation,
            Effect::Ordinary,
            &policy,
            UNIX_EPOCH + day * 30
        ),
        Ok(Decision::Approve { .. })
    ));
}

#[test]
fn grants_commit_and_revoke_atomically_and_reject_unsafe_storage() {
    let scratch = tempfile::tempdir().unwrap();
    let paths = MarsaPaths::from_home(scratch.path()).unwrap();
    create_private(paths.root());
    create_private(&paths.state_dir());
    let uid = nix::unistd::geteuid().as_raw();
    let store = GrantStore::new(paths.clone(), uid);
    assert_eq!(store.load().unwrap().active(UNIX_EPOCH).unwrap().count(), 0);
    let scope = GrantScope::resolve("client", scratch.path()).unwrap();
    let policy = AgentSettings::default();
    let grant = store
        .update(|grants| {
            grants.record_approval(
                scope,
                AgentOperation::PullImage,
                Effect::Ordinary,
                &policy,
                UNIX_EPOCH,
            )
        })
        .unwrap();
    let path = paths.grants_file();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    let valid = fs::read(&path).unwrap();
    let mut forged: serde_json::Value = serde_json::from_slice(&valid).unwrap();
    forged["grants"][0]["operation"] = serde_json::Value::from(AgentOperation::RemoveVolume as i32);
    fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert!(matches!(store.load(), Err(GrantError::InvalidDocument)));
    fs::write(&path, valid).unwrap();
    let denied = store.update(|grants| {
        assert!(grants.revoke(&grant.id));
        Err::<(), _>(GrantError::Ineligible)
    });
    assert!(denied.is_err());
    let reloaded = GrantStore::new(paths.clone(), uid);
    assert_eq!(
        reloaded.load().unwrap().active(UNIX_EPOCH).unwrap().count(),
        1
    );
    assert!(
        reloaded
            .update(|grants| Ok(grants.revoke(&grant.id)))
            .unwrap()
    );
    assert_eq!(store.load().unwrap().active(UNIX_EPOCH).unwrap().count(), 0);

    let alternative = path.with_file_name("redirected");
    fs::rename(&path, &alternative).unwrap();
    symlink(&alternative, &path).unwrap();
    assert!(store.load().is_err());
    assert!(store.update(|_| Ok(())).is_err());
    fs::remove_file(&path).unwrap();
    fs::hard_link(&alternative, &path).unwrap();
    assert!(store.load().is_err());
    fs::remove_file(alternative).unwrap();
    fs::write(&path, "").unwrap();
    assert!(matches!(store.load(), Err(GrantError::InvalidDocument)));
    fs::write(&path, r#"{"version":2,"grants":[]}"#).unwrap();
    assert!(matches!(store.load(), Err(GrantError::InvalidDocument)));
}

fn create_private(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
