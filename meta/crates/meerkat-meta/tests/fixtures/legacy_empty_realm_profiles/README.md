# Legacy empty realm profile store

`realm_profiles.db` is the child-root realm profile database a predecessor
(MobKit 0.8.47) left on a production installation: exactly the table
`realm_profiles` and its primary-key index, zero rows, no schema ledger.
SHA256 `acaa34da9ffae5d5b1e456f75c94f26c9f5c202298a65b759ca3830b48982fe5`.
It holds no data (no rows, no freelist pages). The durable child-mob
composition recognizes exactly this shape; see
`child_mob_storage::recognize_legacy_empty_realm_profiles`.
