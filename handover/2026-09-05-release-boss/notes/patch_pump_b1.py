p='meerkat-mob/src/runtime/event_pump.rs'
s=open(p).read()
def rep(old,new):
    global s
    assert s.count(old)==1,(s.count(old),old[:160]); s=s.replace(old,new)
rep('''    /// After a replacement released `pump_transition` for the bounded join,
    /// another install may have published a pump for the same member. Adopt
    /// it when it matches `material` (the join was redundant), yield to it
    /// when it does not (a newer incarnation won the barrier), and report
    /// `false` only when the slot is still vacant so the caller may install.
    ///
    /// `tap` is the tap-lane subscription to attach on an exact match; the
    /// completion lane passes `None` and publishes its keep-alive instead.
    fn pump_installed_while_barrier_released(
        &self,
        material: &MemberPumpMaterial,
        tap: Option<mpsc::Sender<AttributedEvent>>,
    ) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(existing) = state.pumps.get_mut(&material.agent_identity) else {
            return false;
        };
        if existing.exiting {
            // An exiting entry publishes its own removal under the barrier;
            // it is not a live lease and must not block this install.
            state.pumps.remove(&material.agent_identity);
            return false;
        }
        if existing.expected_member == material.expected_member
            && existing.runtime_id == material.runtime_id
            && existing.peer == material.peer
        {
            match tap {
                Some(tap) => existing.taps.push(tap),
                None => existing.obligation_keepalive = true,
            }
            drop(state);
            self.liveness_changed.notify_waiters();
            return true;
        }
        tracing::debug!(
            agent_identity = %material.agent_identity,
            "member event pump replacement yielded to a newer install published during the join"
        );
        true
    }
''','''    /// After a replacement released `pump_transition` for the bounded join,
    /// another install may have published a pump for the same member. Adopt
    /// it when it matches `material` (the join was redundant), yield to it
    /// when it does not (a newer incarnation won the barrier), and return the
    /// caller's tap in `Err` only when the slot is still vacant so the caller
    /// may install.
    ///
    /// Whichever pump now owns the slot inherits what the replaced entry
    /// carried, exactly as a barrier-serialized replacement would have: its
    /// live taps and completion keep-alive when the expected member is the
    /// same (an address-only refresh), or its waiters are failed when the
    /// incarnation changed. Nothing the old pump held is dropped on the
    /// floor because the barrier was released.
    ///
    /// `tap` is the tap-lane subscription to attach on an exact match; the
    /// completion lane passes `None` and publishes its keep-alive instead.
    fn adopt_pump_installed_while_barrier_released(
        &self,
        material: &MemberPumpMaterial,
        replaced: &mut PumpEntry,
        tap: Option<mpsc::Sender<AttributedEvent>>,
    ) -> Result<(), Option<mpsc::Sender<AttributedEvent>>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(existing) = state.pumps.get_mut(&material.agent_identity) else {
            return Err(tap);
        };
        if existing.exiting {
            // An exiting entry publishes its own removal under the barrier;
            // it is not a live lease and must not block this install.
            state.pumps.remove(&material.agent_identity);
            return Err(tap);
        }
        let same_residency = existing.expected_member == replaced.expected_member;
        if same_residency {
            let inherited = std::mem::take(&mut replaced.taps);
            existing
                .taps
                .extend(inherited.into_iter().filter(|tap| !tap.is_closed()));
            existing.obligation_keepalive |= replaced.obligation_keepalive;
        }
        let exact_match = existing.expected_member == material.expected_member
            && existing.runtime_id == material.runtime_id
            && existing.peer == material.peer;
        if exact_match {
            match tap {
                Some(tap) => existing.taps.push(tap),
                None => existing.obligation_keepalive = true,
            }
        } else {
            tracing::debug!(
                agent_identity = %material.agent_identity,
                "member event pump replacement yielded to a newer install published during the join"
            );
        }
        drop(state);
        if !same_residency {
            self.waiters.fail_all_for(&replaced.expected_member);
        }
        self.liveness_changed.notify_waiters();
        Ok(())
    }
''')
rep('''        let (mut inherited_taps, rewind_same_residency) = if let Some(entry) = replaced {
            entry.cancel.cancel();''','''        let (mut inherited_taps, rewind_same_residency) = if let Some(mut entry) = replaced {
            entry.cancel.cancel();''')
rep('''                transition = self.pump_transition.lock().await;
                self.reap_finished_pump_tasks();
                if self.pump_installed_while_barrier_released(&material, None) {
                    return;
                }
''','''                transition = self.pump_transition.lock().await;
                self.reap_finished_pump_tasks();
                if self
                    .adopt_pump_installed_while_barrier_released(&material, &mut entry, None)
                    .is_ok()
                {
                    return;
                }
''')
rep('''        let (mut inherited_taps, inherited_keepalive, rewind_same_residency) = if let Some(entry) =
            replaced
        {''','''        let (mut inherited_taps, inherited_keepalive, rewind_same_residency) = if let Some(
            mut entry,
        ) = replaced
        {''')
rep('''                transition = self.pump_transition.lock().await;
                self.reap_finished_pump_tasks();
                if self.pump_installed_while_barrier_released(&material, tx.take()) {
                    return rx;
                }
''','''                transition = self.pump_transition.lock().await;
                self.reap_finished_pump_tasks();
                match self.adopt_pump_installed_while_barrier_released(
                    &material,
                    &mut entry,
                    tx.take(),
                ) {
                    Ok(()) => return rx,
                    Err(returned_tap) => tx = returned_tap,
                }
''')
open(p,'w').write(s)
print("event_pump ok")
