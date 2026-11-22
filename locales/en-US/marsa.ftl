### Command-line interface

# Shown when a command is present but cannot yet be run.
cli-not-available = This command is not available yet.

cli-help-print-a-json-response = Print a JSON response
cli-help-confirm-destructive-actions = Confirm destructive actions
cli-help-print-help = Print help
cli-help-settings-key = Settings key
cli-help-approval-request-id = Approval request ID
cli-help-manage-marsa = Manage Marsa
cli-help-show-the-engine-and-vm-status = Show the engine and VM status
cli-help-start-the-engine-vm = Start the engine VM
cli-help-stop-the-engine-vm = Stop the engine VM
cli-help-restart-the-engine-vm = Restart the engine VM
cli-help-print-the-docker-host-export = Print the DOCKER_HOST export
cli-help-manage-settings = Manage settings
cli-help-read-a-setting = Read a setting
cli-help-set-a-setting = Set a setting
cli-help-new-setting-value = New setting value
cli-help-list-settings = List settings
cli-help-manage-shared-folders = Manage shared folders
cli-help-list-shared-folders = List shared folders
cli-help-list-pending-bind-approvals = List pending bind approvals
cli-help-answer-a-pending-bind-request = Answer a pending bind request
cli-help-approval-identifier = Approval identifier
cli-help-share-once-always-or-deny = Share once, always, or deny
cli-help-allow-a-folder-to-be-shared = Allow a folder to be shared
cli-help-folder-to-share = Folder to share
cli-help-remove-a-shared-folder-from-the-allowlist = Remove a shared folder from the allowlist
cli-help-shared-folder-or-id = Shared folder or ID
cli-help-manage-vm-disk-space = Manage VM disk space
cli-help-show-disk-usage = Show disk usage
cli-help-manage-networking = Manage networking
cli-help-show-network-status = Show network status
cli-help-show-or-save-the-network-mode-for-the-next-start = Show or save the network mode for the next start
cli-help-network-mode = Network mode
cli-help-show-daemon-and-guest-logs = Show daemon and guest logs
cli-help-follow-new-log-entries = Follow new log entries
cli-help-collect-a-diagnostics-bundle = Collect a diagnostics bundle
cli-help-manage-app-updates = Manage app updates
cli-help-check-for-updates = Check for updates
cli-help-apply-an-update = Apply an update
cli-help-run-the-mcp-server-on-stdio = Run the MCP server on stdio
cli-help-manage-agent-permissions = Manage agent permissions
cli-help-list-agent-permissions = List agent permissions
cli-help-approve-an-agent-request = Approve an agent request
cli-help-deny-an-agent-request = Deny an agent request
cli-help-revoke-an-agent-grant = Revoke an agent grant
cli-help-agent-grant-id = Agent grant ID
cli-help-reset-marsa-settings-and-optionally-erase-vm-data = Reset Marsa settings and optionally erase VM data
cli-help-erase-all-marsa-settings-and-vm-data = Erase all Marsa settings and VM data
cli-help-show-component-versions = Show component versions
cli-help-generate-shell-completions = Generate shell completions
cli-help-target-shell = Target shell
cli-help-usage = Usage
cli-help-options = Options
cli-help-commands = Commands
cli-help-arguments = Arguments
cli-help-options-placeholder = OPTIONS
cli-help-command-placeholder = COMMAND
cli-help-display-language = Display language (en-US or ar)
cli-invalid-invocation = Invalid invocation. Run marsa --help for usage.
cli-confirmation-required = This operation requires --yes when run non-interactively.
cli-confirmation-declined = Reset canceled. The confirmation did not match.
cli-connection-failed = Could not connect to the Marsa daemon. Open Marsa and try again.
cli-home-unavailable = Could not resolve the socket under the current user's home directory.
cli-unprivileged-user-required = Run Marsa as an unprivileged user.
cli-incompatible-daemon = The daemon API is incompatible with this version of the Marsa CLI.
cli-operation-failed = The Marsa operation failed. Run marsa status or check marsa logs for details.
cli-bind-resolution-failed = Could not answer the bind request. Run marsa share approvals to check whether it is still pending.
cli-write-response-failed = Could not write the response

cli-help-preview-unused-guest-storage-with-dry-run = Preview unused guest storage with --dry-run
cli-help-show-a-read-only-preview = Show a read-only preview
cli-help-include-unused-local-volumes = Include unused local volumes
cli-help-return-free-space-from-a-running-vm-to-macos = Return free space from a running VM to macOS
cli-help-registered-client-id = Registered client ID
cli-help-absolute-project-folder = Absolute project folder
cli-cleanup-apply-unavailable = Cleanup apply is unavailable. Run marsa disk cleanup --dry-run to preview.
cli-cleanup-preview = Could not preview unused storage. Disk cleanup preview requires a running engine.
cli-reclaim = Could not reclaim disk space. Reclaim requires a running VM.
cli-daemon-unavailable = Could not connect to the local Marsa daemon.
cli-agent-request-failed = Marsa could not complete the agent command.
cli-agent-approval-missing = No pending approval matches that ID.
cli-agent-grant-missing = No active grant matches that ID.
cli-agent-review-required = Approving an agent request requires reviewing and confirming it in a terminal.
cli-mcp-unavailable = Could not start marsa-mcp beside the Marsa CLI.
cli-agent-review-declined = The confirmation was declined.

## Disk

cli-disk-host-allocation = Host allocation (bytes) { $bytes }
cli-disk-host-capacity = Host image capacity (bytes) { $bytes }
cli-disk-guest-used = Guest filesystem used (bytes) { $bytes }
cli-disk-guest-capacity = Guest filesystem capacity (bytes) { $bytes }
cli-disk-guest-unavailable = Guest filesystem usage is unavailable
cli-disk-containers = Containers
cli-disk-images = Images
cli-disk-volumes = Volumes
cli-disk-networks = Networks
cli-disk-build-cache = Build cache
cli-disk-other = Other disk usage
cli-disk-category-usage = { $category } { $used } used bytes, { $reclaimable } reclaimable bytes

## Engine

cli-engine-cpu-time = Cumulative CPU time (ns) { $value }
cli-engine-memory = Memory (bytes) { $value }
cli-engine-disk = Guest disk (bytes) { $value }
cli-engine-cli-version = Marsa CLI { $value }
cli-engine-daemon-version = Marsa daemon { $value }
cli-engine-api-version = Daemon API { $value }
cli-engine-starting-engine = Starting the engine…
cli-engine-stopping-engine = Stopping the engine…
cli-engine-restarting-engine = Restarting the engine…
cli-engine-state = Engine state { $state }
cli-engine-backend = VM backend { $value }
cli-engine-guest-health = Guest engine health { $state }
cli-engine-state-stopped = Stopped
cli-engine-state-starting = Starting
cli-engine-state-booting = Booting the virtual machine
cli-engine-state-ready = Ready
cli-engine-state-idle = Idle
cli-engine-state-paused = Paused
cli-engine-state-stopping = Stopping
cli-engine-state-failed = Failed
cli-engine-state-unknown = Unknown
cli-engine-health-unhealthy = Unhealthy
cli-engine-health-restarting = Restarting

## Maintenance

cli-maintenance-diagnose-path = Diagnostics archive created at { $path }
cli-maintenance-factory-reset-completed = Factory reset completed.
cli-maintenance-settings-reset-completed = Marsa settings reset.
cli-maintenance-factory-reset-prompt = This erases Marsa settings and VM data. Type { $phrase } to continue.
cli-maintenance-settings-reset-prompt = This resets Marsa settings. Type { $phrase } to continue.

## Networking

cli-network-restart-pending = Restart pending. The saved mode will take effect on the next start.
cli-network-restart-current = No restart is pending.
cli-network-restart-unknown = Restart status is unknown.
cli-network-connectivity = Connectivity check { $value }
cli-network-guest-address = Guest IPv4 { $value }
cli-network-gateway-address = Gateway IPv4 { $value }
cli-network-configured-mode = Configured mode { $mode }
cli-network-active-mode = Active mode { $mode }
cli-network-automatic = Automatic
cli-network-nat = Native NAT
cli-network-userspace = User-space
cli-network-unavailable = Unavailable

## Settings

cli-settings-saved-restart = Saved. Restart the VM to apply the change.
cli-settings-saved = Saved.
cli-settings-rejected = The Marsa daemon rejected the request.
cli-settings-rejected-with-detail = The Marsa daemon rejected the request. { $detail }

## Bind approvals

cli-share-bind-approved = Share approved. Run marsa restart when your workloads can stop, then retry the Docker command.
cli-share-bind-denied = Bind request denied.
cli-share-no-approvals = No pending bind approvals.

## Shared folders

cli-share-folders-empty = No shared folders.

## Configuration validation

cli-config-too-large = settings document exceeds the { $bytes } byte limit
cli-config-invalid-toml = invalid settings TOML at line { $line }, column { $column }
cli-config-version = unsupported settings schema version { $version }
cli-config-cpus = invalid setting { $field }: must be at least one
cli-config-memory = invalid setting { $field }: must be at least 512 MiB and fit in a signed 64-bit byte count
cli-config-idle = invalid setting { $field }: must be at least one second
cli-config-disk-limit = invalid setting { $field }: must be at least 1 GiB and fit in a signed 64-bit byte count
cli-config-proxy = invalid setting { $field }: must be an HTTP or HTTPS proxy URL without credentials, a path, query, or fragment
cli-config-telemetry = invalid setting { $field }: must be false because telemetry is unavailable
cli-config-invalid = invalid setting { $field }. Check the allowed values.
cli-config-stale = settings revision changed; load the latest settings
cli-config-proxy-enabled = disable the approved daemon proxy before changing network mode
cli-config-encode = could not encode settings as TOML
cli-config-share-path = share paths must be absolute, non-root UTF-8 paths without parent traversal or NUL bytes
cli-config-share-resolve = could not resolve the host path
cli-config-share-protection = could not verify protected host paths
cli-config-share-protected = the host path overlaps a protected directory or credential store
cli-config-share-directory = shared folders must be directories
cli-config-share-source = bind sources must be regular files or directories
cli-config-share-not-allowed = the host path is outside the shared folders
cli-config-share-layout = The shared folders exceed the supported guest location, count or path length.
cli-config-share-unknown = unknown share ID

## Disk reclaim and cleanup preview

cli-disk-reclaim-host = Host space reclaimed (bytes) { $bytes }
cli-disk-reclaim-guest = Guest space submitted for discard (bytes) { $bytes }
cli-disk-preview-heading = Read-only cleanup preview
cli-disk-preview-estimate-unavailable = estimate unavailable
cli-disk-preview-bytes = { $size } bytes
cli-disk-preview-item = { $name }  { $estimate }
cli-disk-preview-missing-inventory = { $category }  inventory unavailable
cli-disk-preview-total = Estimated guest bytes  { $estimate }

## Agents

cli-agents-clients = Clients
cli-agents-grants = Grants
cli-agents-pending = Pending requests
cli-agents-approve-prompt = Approve this request once?
cli-agents-approved = Request approved.
cli-agents-denied = Request denied.
cli-agents-revoke-prompt = Revoke this grant?
cli-agents-revoked = Grant revoked.
cli-agents-confirm-instructions = Type { $answer } and press { $enter }.
