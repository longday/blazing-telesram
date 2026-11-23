# Spec Delta

## REMOVED Requirements

### Requirement: Read-only client
**Reason**: The client now sends messages, edits, deletions, read markers and typing on the user's behalf.
**Migration**: The remaining write restrictions are the `message-sending` requirement "Writes only from user actions"; when reading marks a chat read is defined by `read-markers`.
