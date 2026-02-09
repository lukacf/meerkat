// Generated wire types for Meerkat SDK
// Contract version: 0.1.0

export const CONTRACT_VERSION = "0.1.0";

export interface WireUsage {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cache_creation_tokens?: number;
  cache_read_tokens?: number;
}

export interface WireRunResult {
  session_id: string;
  text: string;
  turns: number;
  tool_calls: number;
  usage: WireUsage;
  structured_output?: unknown;
  schema_warnings?: Array<{ provider: string; path: string; message: string }>;
}

export interface WireEvent {
  session_id: string;
  sequence: number;
  event: Record<string, unknown>;
  contract_version: string;
}

export interface CapabilityEntry {
  id: string;
  description: string;
  status: string;
}

export interface CapabilitiesResponse {
  contract_version: string;
  capabilities: CapabilityEntry[];
}
