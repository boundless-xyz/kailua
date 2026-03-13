## ADDED Requirements

### Requirement: FPVM Upgrade book page exists
A new page `fpvm-upgrade.md` SHALL be added to `book/src/` and linked under "On-chain" in `SUMMARY.md`.

#### Scenario: Page appears in book navigation
- **WHEN** the book is built
- **THEN** "FPVM Upgrade" appears as a sub-item under "On-chain" in the sidebar

### Requirement: Documentation covers proxy architecture
The page SHALL include a description of the proxy pattern used (OP Stack Proxy, EIP-1967) and explain what persists across upgrades (faultProofPermits storage) versus what changes (immutable config values in the new implementation).

#### Scenario: Architecture section present
- **WHEN** a reader opens the FPVM Upgrade page
- **THEN** the page explains the proxy architecture and the distinction between proxy storage and implementation bytecode

### Requirement: Documentation covers upgrade prerequisites
The page SHALL list prerequisites: proxy admin access (private key or Safe), the proxy address, and the new parameter values.

#### Scenario: Prerequisites section present
- **WHEN** a reader opens the FPVM Upgrade page
- **THEN** prerequisites are listed with env var names and descriptions

### Requirement: Documentation covers running the upgrade
The page SHALL include the `forge script` command to run `UpgradeVerifier.s.sol` with required and optional env vars.

#### Scenario: Upgrade command present
- **WHEN** a reader opens the FPVM Upgrade page
- **THEN** a complete forge script command is shown with env var placeholders

### Requirement: Documentation covers verification
The page SHALL include `cast call` commands to verify the new implementation's values through the proxy after upgrade.

#### Scenario: Verification commands present
- **WHEN** a reader opens the FPVM Upgrade page
- **THEN** cast commands are shown for reading FPVM_IMAGE_ID and other values through the proxy to confirm the upgrade succeeded
