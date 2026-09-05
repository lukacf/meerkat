impl = open('/tmp/rb/store_impl.rs').read()
impl = "\n".join(line[4:] if line.startswith("    ") else line for line in impl.splitlines())
impl = impl.replace("BlockingDestroyCommitStore", "FaultInjectingRuntimeStore")
impl = impl.replace("crate::store::", "meerkat_runtime::store::")
impl = impl.replace("crate::input_state::", "meerkat_runtime::input_state::")
impl = impl.replace("crate::ops_lifecycle::", "meerkat_runtime::ops_lifecycle::")
impl = impl.replace("crate::IdempotencyKey", "meerkat_runtime::IdempotencyKey")
old = '''        let runtime_state = commit.runtime_state();
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await?;
        if runtime_state == RuntimeState::Destroyed {
            self.destroy_commit_started.notify_one();
            self.release_destroy_commit.notified().await;
        }
        Ok(())'''
assert impl.count(old)==1
impl = impl.replace(old, '''        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await''')
old = '''    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .atomic_apply(
                runtime_id,
                session_delta,
                receipt,
                input_updates,
                session_store_key,
            )
            .await
    }'''
assert impl.count(old)==1
impl = impl.replace(old, '''    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.fail_commit_if_flagged(runtime_id, "atomic_apply")?;
        self.inner
            .atomic_apply(
                runtime_id,
                session_delta,
                receipt,
                input_updates,
                session_store_key,
            )
            .await
    }''')
old = '''    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .persist_input_states_atomically(runtime_id, states)
            .await
    }'''
assert impl.count(old)==1
impl = impl.replace(old, '''    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.park_admission_if_flagged(runtime_id).await;
        self.inner
            .persist_input_states_atomically(runtime_id, states)
            .await
    }''')
old = '''    fn input_state_batch_cas_implementation_profile('''
assert impl.count(old)==1
impl = impl.replace(old, '''    async fn commit_prepared_session_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        request: meerkat_runtime::store::PreparedRuntimeSessionCommit,
    ) -> Result<
        meerkat_runtime::store::PreparedRuntimeSessionCommitResult,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.fail_commit_if_flagged(runtime_id, "commit_prepared_session_boundary")?;
        self.inner
            .commit_prepared_session_boundary(runtime_id, request)
            .await
    }

    fn input_state_batch_cas_implementation_profile(''')

header = open('/tmp/rb/tests_header.rs').read()
tests = open('/tmp/rb/tests_body.rs').read()
open('meerkat-mob/src/runtime/tests/actor_isolation.rs','w').write(header + impl + "\n" + tests)
print("written", len(header)+len(impl)+len(tests))
