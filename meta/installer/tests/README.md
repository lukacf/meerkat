# Forge hosted MCP tests

Run the hosted MCP tests from the repository root in a prepared Forge test
environment, with the reviewed Connect Python source directory and matching
Meta Python SDK selected explicitly. `CONNECT_SDK_SOURCE` is the directory
containing `meerkat_toolkit`; `META_SDK_SOURCE` contains `meerkat_meta`.

```sh
export PYTHONPATH="installer:${CONNECT_SDK_SOURCE:?Select the reviewed Connect SDK}:${META_SDK_SOURCE:?Select its matching Meta SDK}"
python -c 'import meerkat_toolkit.mcp_auth_native as c, meerkat_meta as m; print(c.__file__); print(m.__file__)'
python -m pytest -ra \
  installer/tests/test_console_hosted_mcp.py \
  installer/tests/test_hosted_mcp_startup.py \
  installer/tests/test_native_console_login.py
```

Check the printed module paths against the selected SDK sources. A missing
Connect package causes the pipe tests to skip; a skipped suite is not evidence
that the integration works. Keep the selected SDK revisions with the result.

The pipe tests use the real Python SDK with an inert peer and ASGI requests.
They cover callback routing and browser admission, one retained attempt per
login, cancellation during start/completion/cleanup, and joining cleanup before
retiring the original login. The peer's replies are synthetic. These tests do
not prove provider login, a public TLS route, native resolution or persistence.

Cancellation tests wait for the relevant operation to enter a controlled
boundary, then withhold its reply. The timeout bounds a hung test; elapsed time
does not establish that the operation entered. Keep the shared pipe open until
the application has joined its own cleanup, so transport closure cannot make a
missing join appear successful.

For expected cleanup failures, pass `cleanup_error` to the pipe `boundary`
fixture. Its `pytest.raises` covers only `front.close()`. Wrapping the whole
fixture body in an expected-error context can hide a failed assertion when
teardown raises that expected error. For changes to those assertions, a useful
negative check is to temporarily insert an impossible assertion in the body:
the affected test must fail, even when cleanup raises its expected error.
Restore the test before running the passing suite.
