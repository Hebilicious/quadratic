import * as App from "@acid-cat/core-effect/App";
import * as Capability from "@acid-cat/core-effect/Capability";
import * as Event from "@acid-cat/core-effect/Event";
import * as Session from "@acid-cat/core-effect/Session";
import { CallFailureError, isCallFailure } from "@acid-cat/spec-ts";
import type { CatalogDocument, Emitted, JsonObject, Occurrence } from "@acid-cat/spec-ts";
import { Effect, SchemaRepresentation, Stream } from "effect";
import type { JsonSchema } from "effect";

import shared from "quadratique-catalog/quadratique.acidcat.json" with { type: "json" };
import { Server } from "./server.ts";

export const sharedCatalog: CatalogDocument = shared;

export const PAGE_TARGETS: readonly string[] = ["web", "webmcp"];

const onThePage = (audience: readonly string[] | undefined): boolean =>
  (audience ?? []).some((target) => PAGE_TARGETS.includes(target));

const NO_INPUT: JsonObject = { type: "object", properties: {} };
const NO_OUTPUT: JsonObject = { type: "null" };

const capabilityEntries = Object.entries(sharedCatalog.capabilities).filter(([, capability]) =>
  onThePage(capability.audience),
);
const eventEntries = Object.entries(sharedCatalog.events ?? {}).filter(([, event]) =>
  onThePage(event.audience),
);

const capabilityRoots = capabilityEntries.map(([, capability]) => [
  capability.input ?? NO_INPUT,
  capability.output ?? NO_OUTPUT,
  ...Object.values(capability.failures ?? {}),
]);
const eventRoots = eventEntries.map(([, event]) => [
  event.input ?? NO_INPUT,
  event.payload ?? NO_INPUT,
]);

function regrouped<T>(flat: readonly T[], groups: readonly (readonly unknown[])[]): T[][] {
  let start = 0;
  return groups.map((group) => {
    const slice = flat.slice(start, start + group.length);
    start += group.length;
    return slice;
  });
}

/**
 * BOUNDARY: the catalog is our own approved document, so its patterns are applied. Each imported schema decodes JSON
 * into JSON and needs no service, which is what `AnySchema` states.
 */
const imported = SchemaRepresentation.fromJsonSchemaMultiDocument(
  {
    dialect: "draft-2020-12",
    schemas: [...capabilityRoots, ...eventRoots].flat() as unknown as [
      JsonSchema.JsonSchema,
      ...JsonSchema.JsonSchema[],
    ],
    definitions: (sharedCatalog.$defs ?? {}) as JsonSchema.Definitions,
  },
  { patterns: "apply" },
) as unknown as readonly Capability.AnySchema[];
const capabilitySchemas = regrouped(imported, capabilityRoots);
const eventSchemas = regrouped(imported.slice(capabilityRoots.flat().length), eventRoots);

const forward = (id: string) => (input: unknown) =>
  Effect.gen(function* () {
    const target = yield* Session.Target;
    const server = yield* Server;
    const outcome = yield* server.client(target).invoke(id, input);
    if (isCallFailure(outcome)) {
      return yield* Effect.die(new CallFailureError(outcome));
    }
    if (outcome.ok) {
      return outcome.value;
    }
    return yield* Effect.fail(outcome.error);
  });

const pageCapabilities = capabilityEntries.map(([id, declared], index) => {
  const [input, output, ...failures] = capabilitySchemas[index] ?? [];
  if (input === undefined || output === undefined) {
    throw new Error(`The schemas of the capability ${id} were not imported`);
  }
  return Capability.make({
    id,
    description: declared.description,
    audience: declared.audience ?? [],
    ...(declared.readOnly === true ? { readOnly: true } : {}),
    input,
    output,
    failures,
    run: forward(id),
  });
});

const emitted = (occurrence: Occurrence): Emitted => ({
  data: occurrence.data,
  eventId: occurrence.eventId,
  timestamp: occurrence.timestamp,
  ...(occurrence.cursor === undefined ? {} : { cursor: occurrence.cursor }),
  ...(occurrence.truncated === true ? { truncated: true } : {}),
});

const pageEvents = eventEntries.map(([id, declared], index) => {
  const [input, payload] = eventSchemas[index] ?? [];
  if (input === undefined || payload === undefined) {
    throw new Error(`The schemas of the event ${id} were not imported`);
  }
  return Event.make({
    id,
    description: declared.description,
    audience: declared.audience ?? [],
    input,
    payload,
    subscribe: (subscription, resume) =>
      Stream.unwrap(
        Effect.gen(function* () {
          const target = yield* Session.Target;
          const server = yield* Server;
          const occurrences = yield* server.client(target).subscribe(id, subscription, resume);
          server.listenerRegistered(target, id);
          return Stream.orDie(Stream.map(occurrences, emitted));
        }).pipe(Effect.orDie),
      ),
  });
});

export const app = App.make({
  title: sharedCatalog.title ?? "Quadratique",
  ...(sharedCatalog.description === undefined ? {} : { description: sharedCatalog.description }),
  targets: sharedCatalog.targets,
  capabilities: pageCapabilities,
  events: pageEvents,
});
