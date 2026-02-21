/**
 * 008 — Structured Output (TypeScript SDK)
 *
 * Force the agent to return JSON conforming to a specific schema.
 * This is essential for building reliable pipelines where downstream code
 * needs to parse the agent's output programmatically.
 *
 * What you'll learn:
 * - Defining an output schema (JSON Schema)
 * - Using `output_schema` with `createSession()`
 * - Parsing the structured result
 * - Handling validation retries
 *
 * Run:
 *   ANTHROPIC_API_KEY=sk-... npx tsx main.ts
 */

import { MeerkatClient } from "meerkat-sdk";

// Define the schema for the agent's output
const sentimentSchema = {
  type: "object" as const,
  properties: {
    sentiment: {
      type: "string" as const,
      enum: ["positive", "negative", "neutral", "mixed"],
      description: "Overall sentiment classification",
    },
    confidence: {
      type: "number" as const,
      minimum: 0,
      maximum: 1,
      description: "Confidence score between 0 and 1",
    },
    key_phrases: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Key phrases that influenced the classification",
    },
    summary: {
      type: "string" as const,
      description: "One-sentence summary of the text",
    },
  },
  required: ["sentiment", "confidence", "key_phrases", "summary"],
  additionalProperties: false,
};

interface SentimentResult {
  sentiment: "positive" | "negative" | "neutral" | "mixed";
  confidence: number;
  key_phrases: string[];
  summary: string;
}

async function analyzeSentiment(
  client: MeerkatClient,
  text: string
): Promise<SentimentResult> {
  const result = await client.createSession(
    `Analyze the sentiment of the following text:\n\n"${text}"`,
    {
      model: "claude-sonnet-4-5",
      system_prompt:
        "You are a sentiment analysis engine. Analyze text and return structured results.",
      output_schema: sentimentSchema,
      structured_output_retries: 3, // Retry up to 3 times if schema validation fails
    }
  );

  return JSON.parse(result.text) as SentimentResult;
}

async function main() {
  const client = new MeerkatClient();
  await client.connect();

  try {
    const texts = [
      "The new release is incredible! Performance improved 3x and the API is much cleaner.",
      "The server has been down for hours. Support is unresponsive. Very disappointed.",
      "The weather today is partly cloudy with temperatures around 72°F.",
      "I love the design but the price is way too high for what you get.",
    ];

    console.log("=== Sentiment Analysis Pipeline ===\n");

    for (const text of texts) {
      const analysis = await analyzeSentiment(client, text);
      console.log(`Text: "${text.substring(0, 60)}..."`);
      console.log(`  Sentiment:   ${analysis.sentiment}`);
      console.log(`  Confidence:  ${(analysis.confidence * 100).toFixed(0)}%`);
      console.log(`  Key phrases: ${analysis.key_phrases.join(", ")}`);
      console.log(`  Summary:     ${analysis.summary}`);
      console.log();
    }
  } finally {
    await client.close();
  }
}

main().catch(console.error);
