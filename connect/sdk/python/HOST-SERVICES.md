# Selected host-service handles

`meerkat_toolkit.host_services.StagedHostServices` provides the shared narrow
binding step for custom extensions. It accepts declared service names and the
available subset already selected by the host. It does not parse a manifest,
negotiate versions or evaluate policy.

The host first uses the canonical package inspector's `activation-host` report
and rejects required gaps across the entire selection. It verifies current
package/artifact, configuration and policy bindings and handles any optional
degradation it supports. It then constructs one view per extension and binds
exactly the available names to already scoped service handles.

```python
view = StagedHostServices(declared={"example/media"}, available={"example/media"})
view.bind({"example/media": scoped_media_handle})
media = view.get("example/media")
```

`bind` is single-assignment. Missing, extra and `None` handles are refused.
`get` distinguishes undeclared, unavailable and not-yet-bound services through
`HostServiceError.kind`. Changing caller-owned sets or maps after construction
cannot retarget the view. `ready` means the view has handles, not that the package
or its exports are ready.

The original handle must enforce revocation on every call, including when an
extension retains it. This Python object is a cooperative API boundary, not a
sandbox for malicious in-process code. The host still owns process isolation,
authorization, policy changes and lifetime. A different selection requires a new
view and retirement of the old authority by those owners.
