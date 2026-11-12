/**
 * `MobHandle.wireCrossMob`: the calls it makes, in order.
 *
 * `peerInfo` on both sides, one read of the local member (is it wired to
 * that peer already), then `wireLocal` on each side with the other
 * side's answer: by the dialable address and the member's key when BOTH
 * answers report a dialable address (two gateways, two processes), and by the
 * in-process address with no key otherwise (the gateway resolves a member of
 * its own process, and refuses any other in plain words). A refused second
 * `wireLocal` undoes the first side with the parameters it was wired with,
 * unless that side was wired to the peer before the call.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { MetaRuntime } from "../src/runtime.js";
import { runtimeConfig } from "./runtime-config.js";

interface RpcCall {
  method: string;
  params?: Record<string, unknown>;
}

function peer(
  mob: string,
  member: string,
  peerId: string,
  socket?: { address: string },
): Record<string, string> {
  const commsName = `${mob}/worker/${member}`;
  return {
    member_id: member,
    mob_id: mob,
    comms_name: commsName,
    peer_id: peerId,
    address: `inproc://${commsName}`,
    pubkey_b64: `${member}-key`,
    ...(socket ? { dialable_address: socket.address } : {}),
  };
}

function mockHandle(
  info: Record<string, string>,
  refuse: string[] = [],
  wiredTo: string[] = [],
): { handle: any; calls: RpcCall[] } {
  const rt = new MetaRuntime(runtimeConfig());
  (rt as any)._running = true;
  const calls: RpcCall[] = [];
  (rt as any)._rpc = async (method: string, params?: Record<string, unknown>) => {
    calls.push({ method, params });
    if (refuse.includes(method)) {
      throw new Error(`${method} refused`);
    }
    if (method === "meta/get_member") {
      return { agent_identity: info.member_id, role: "worker", state: "active", wired_to: wiredTo };
    }
    return method === "meta/cross_mob/peer_info" ? info : {};
  };
  return { handle: rt.mobHandle(), calls };
}

function inProcessWire(localMember: string, remote: Record<string, string>) {
  return {
    local_member_id: localMember,
    remote_comms_name: remote.comms_name,
    remote_peer_id: remote.peer_id,
    remote_address: remote.address,
  };
}

function socketWire(localMember: string, remote: Record<string, string>) {
  return {
    local_member_id: localMember,
    remote_comms_name: remote.comms_name,
    remote_peer_id: remote.peer_id,
    remote_address: remote.dialable_address,
    remote_pubkey_b64: remote.pubkey_b64,
  };
}

const ALICE_ID = "00000000-0000-4000-8000-00000000000a";
const BOB_ID = "00000000-0000-4000-8000-00000000000b";

describe("MobHandle.wireCrossMob", () => {
  it("wires each side by the other's socket endpoint and key when both have one", async () => {
    const alice = peer("mob-a", "alice", ALICE_ID, { address: "tcp://127.0.0.1:4101" });
    const bob = peer("mob-b", "bob", BOB_ID, { address: "tcp://127.0.0.1:4102" });
    const local = mockHandle(alice);
    const remote = mockHandle(bob);

    await local.handle.wireCrossMob("alice", "bob", remote.handle);

    assert.deepEqual(local.calls, [
      { method: "meta/cross_mob/peer_info", params: { member_id: "alice" } },
      { method: "meta/get_member", params: { member_id: "alice" } },
      { method: "meta/cross_mob/wire_local", params: socketWire("alice", bob) },
    ]);
    assert.deepEqual(remote.calls, [
      { method: "meta/cross_mob/peer_info", params: { member_id: "bob" } },
      { method: "meta/cross_mob/wire_local", params: socketWire("bob", alice) },
    ]);
  });

  it("wires by the in-process address with no key when neither has a socket endpoint", async () => {
    const alice = peer("mob-a", "alice", ALICE_ID);
    const bob = peer("mob-b", "bob", BOB_ID);
    const local = mockHandle(alice);
    const remote = mockHandle(bob);

    await local.handle.wireCrossMob("alice", "bob", remote.handle);

    assert.deepEqual(local.calls[2], {
      method: "meta/cross_mob/wire_local",
      params: inProcessWire("alice", bob),
    });
    assert.deepEqual(remote.calls[1], {
      method: "meta/cross_mob/wire_local",
      params: inProcessWire("bob", alice),
    });
  });

  it("never mixes: one socket endpoint alone wires both sides in-process, with no key", async () => {
    const alice = peer("mob-a", "alice", ALICE_ID, { address: "tcp://127.0.0.1:4101" });
    const bob = peer("mob-b", "bob", BOB_ID);
    const local = mockHandle(alice);
    const remote = mockHandle(bob);

    await local.handle.wireCrossMob("alice", "bob", remote.handle);

    assert.deepEqual(local.calls[2].params, inProcessWire("alice", bob));
    assert.deepEqual(remote.calls[1].params, inProcessWire("bob", alice));
  });

  it("a refused first wire is thrown and nothing else is sent", async () => {
    const local = mockHandle(peer("mob-a", "alice", ALICE_ID), ["meta/cross_mob/wire_local"]);
    const remote = mockHandle(peer("mob-b", "bob", BOB_ID));

    await assert.rejects(
      local.handle.wireCrossMob("alice", "bob", remote.handle),
      /wire_local refused/,
    );

    assert.deepEqual(
      local.calls.map((call) => call.method),
      ["meta/cross_mob/peer_info", "meta/get_member", "meta/cross_mob/wire_local"],
    );
    assert.deepEqual(
      remote.calls.map((call) => call.method),
      ["meta/cross_mob/peer_info"],
    );
  });

  it("a refused second wire undoes the first side with the parameters it was wired with", async () => {
    const alice = peer("mob-a", "alice", ALICE_ID, { address: "tcp://127.0.0.1:4101" });
    const bob = peer("mob-b", "bob", BOB_ID, { address: "tcp://127.0.0.1:4102" });
    const local = mockHandle(alice);
    const remote = mockHandle(bob, ["meta/cross_mob/wire_local"]);

    await assert.rejects(
      local.handle.wireCrossMob("alice", "bob", remote.handle),
      /wire_local refused/,
    );

    assert.deepEqual(local.calls, [
      { method: "meta/cross_mob/peer_info", params: { member_id: "alice" } },
      { method: "meta/get_member", params: { member_id: "alice" } },
      { method: "meta/cross_mob/wire_local", params: socketWire("alice", bob) },
      { method: "meta/cross_mob/unwire_local", params: socketWire("alice", bob) },
    ]);
    assert.deepEqual(
      remote.calls.map((call) => call.method),
      ["meta/cross_mob/peer_info", "meta/cross_mob/wire_local"],
    );
  });

  it("a refused second wire leaves a first side that was wired before the call", async () => {
    const alice = peer("mob-a", "alice", ALICE_ID, { address: "tcp://127.0.0.1:4101" });
    const bob = peer("mob-b", "bob", BOB_ID, { address: "tcp://127.0.0.1:4102" });
    // alice is wired to bob already when the helper runs.
    const local = mockHandle(alice, [], [bob.comms_name]);
    const remote = mockHandle(bob, ["meta/cross_mob/wire_local"]);

    await assert.rejects(
      local.handle.wireCrossMob("alice", "bob", remote.handle),
      /wire_local refused/,
    );

    // The helper undoes only a wire it made: no unwire_local is sent.
    assert.deepEqual(local.calls, [
      { method: "meta/cross_mob/peer_info", params: { member_id: "alice" } },
      { method: "meta/get_member", params: { member_id: "alice" } },
      { method: "meta/cross_mob/wire_local", params: socketWire("alice", bob) },
    ]);
  });
});
