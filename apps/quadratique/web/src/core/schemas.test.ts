import { Schema } from "effect";
import { describe, expect, test } from "vitest";

import {
  FailureSchema,
  HistoryChangeSchema,
  IssueAnswerSchema,
  IssueChangedSchema,
  IssueRelationsSchema,
  IssueSchema,
  IssueSummaryAnswerSchema,
  IssueSummarySchema,
  ProjectSchema,
  RelationSchema,
} from "../board/model.ts";
import { sharedCatalog } from "./catalog.ts";

type Json = Readonly<Record<string, unknown>>;

const isJson = (value: unknown): value is Json =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const json = (value: unknown): Json => (isJson(value) ? value : {});

const list = (value: unknown): readonly unknown[] => (Array.isArray(value) ? value : []);

const definitions = json(sharedCatalog.$defs);

function resolved(schema: Json): Json {
  const reference = schema["$ref"];
  if (typeof reference === "string") {
    return resolved(json(definitions[reference.replace("#/$defs/", "")]));
  }
  return schema;
}

function literals(schema: Json): readonly unknown[] | undefined {
  if ("const" in schema) {
    return [schema["const"]];
  }
  return Array.isArray(schema["enum"]) ? schema["enum"] : undefined;
}

function drift(page: Json, catalogSchema: Json, at: string): readonly string[] {
  const catalog = resolved(catalogSchema);
  const variants = list(page["anyOf"]).map(json);
  if (variants.length > 0) {
    const choices = [...list(catalog["oneOf"]), ...list(catalog["anyOf"])].map(json);
    const candidates = choices.length > 0 ? choices : [catalog];
    return variants.flatMap((variant, index) =>
      candidates.some((candidate) => drift(variant, candidate, at).length === 0)
        ? []
        : [`${at}: variant ${String(index)} matches no variant of the catalog`],
    );
  }
  const pageLiterals = literals(page);
  if (pageLiterals !== undefined) {
    const catalogLiterals = literals(catalog) ?? [];
    return JSON.stringify(pageLiterals.toSorted()) ===
      JSON.stringify([...catalogLiterals].toSorted())
      ? []
      : [
          `${at}: the page knows ${JSON.stringify(pageLiterals)}, the catalog ${JSON.stringify(catalogLiterals)}`,
        ];
  }
  const type = page["type"];
  const catalogType = catalog["type"];
  const numeric = (each: unknown) => each === "number" || each === "integer";
  if (
    typeof type === "string" &&
    catalogType !== undefined &&
    type !== catalogType &&
    !(type === "number" && numeric(catalogType))
  ) {
    return [`${at}: the page reads a ${type}, the catalog gives ${String(catalogType)}`];
  }
  if (type === "array") {
    return drift(json(page["items"]), json(catalog["items"]), `${at}[]`);
  }
  if (type !== "object") {
    return [];
  }
  const properties = json(page["properties"]);
  const catalogProperties = json(catalog["properties"]);
  const catalogRequired = list(catalog["required"]);
  return [
    ...list(page["required"]).flatMap((name) =>
      catalogRequired.includes(name) ? [] : [`${at}.${String(name)}: the catalog may leave it out`],
    ),
    ...Object.entries(properties).flatMap(([name, schema]) =>
      name in catalogProperties
        ? drift(json(schema), json(catalogProperties[name]), `${at}.${name}`)
        : [`${at}.${name}: the catalog has no such member`],
    ),
  ];
}

const pageSchema = (schema: Schema.Top): Json => json(Schema.toJsonSchemaDocument(schema).schema);

const comparedOnTheirOwn: Readonly<Record<string, Schema.Top>> = {
  Issue: IssueSchema,
  IssueSummary: IssueSummarySchema,
  Relation: RelationSchema,
};

function wrapperDrift(page: Json, catalogSchema: Json): readonly string[] {
  const catalog = resolved(catalogSchema);
  const catalogProperties = json(catalog["properties"]);
  const catalogRequired = list(catalog["required"]);
  return [
    ...list(page["required"]).flatMap((name) =>
      catalogRequired.includes(name) ? [] : [`.${String(name)}: the catalog may leave it out`],
    ),
    ...Object.entries(json(page["properties"])).flatMap(([name, schema]) => {
      if (!(name in catalogProperties)) {
        return [`.${name}: the catalog has no such member`];
      }
      const member = json(catalogProperties[name]);
      const listed = member["type"] === "array";
      const reference = json(listed ? member["items"] : member)["$ref"];
      const definition =
        typeof reference === "string" ? reference.replace("#/$defs/", "") : undefined;
      const owner = definition === undefined ? undefined : comparedOnTheirOwn[definition];
      if (owner === undefined) {
        return drift(json(schema), member, `.${name}`);
      }
      const read = listed ? json(json(schema)["items"]) : json(schema);
      return JSON.stringify(read) === JSON.stringify(pageSchema(owner))
        ? []
        : [`.${name}: the page does not read it as ${String(definition)}`];
    }),
  ];
}

const failureOf = (tag: string): Json => {
  for (const capability of Object.values(sharedCatalog.capabilities)) {
    const failure = capability.failures?.[tag];
    if (failure !== undefined) {
      return json(failure);
    }
  }
  return {};
};

describe("the schemas the page reads answers with agree with the shared catalog", () => {
  const cases: readonly (readonly [string, Schema.Top, Json])[] = [
    ["IssueSummary", IssueSummarySchema, { $ref: "#/$defs/IssueSummary" }],
    ["Issue", IssueSchema, { $ref: "#/$defs/Issue" }],
    ["Relation", RelationSchema, { $ref: "#/$defs/Relation" }],
    ["Change", HistoryChangeSchema, { $ref: "#/$defs/Change" }],
    ["Project", ProjectSchema, { $ref: "#/$defs/Project" }],
    ["issue.changed", IssueChangedSchema, json(sharedCatalog.events?.["issue.changed"]?.payload)],
  ];

  test.each(cases)("%s asks for nothing the catalog does not promise", (_name, schema, catalog) => {
    expect(drift(pageSchema(schema), catalog, "")).toEqual([]);
  });

  const answers: readonly (readonly [string, Schema.Top, Json])[] = [
    ["IssueAnswer", IssueAnswerSchema, { $ref: "#/$defs/IssueAnswer" }],
    ["IssueSummaryAnswer", IssueSummaryAnswerSchema, { $ref: "#/$defs/IssueSummaryAnswer" }],
    [
      "issue.create",
      IssueSummaryAnswerSchema,
      json(sharedCatalog.capabilities["issue.create"]?.output),
    ],
    ["issue.link", IssueRelationsSchema, json(sharedCatalog.capabilities["issue.link"]?.output)],
    ["issue.close", IssueAnswerSchema, json(sharedCatalog.capabilities["issue.close"]?.output)],
  ];

  test.each(answers)(
    "the answer %s carries each member the page reads, of the kind the page reads it as",
    (_name, schema, catalog) => {
      expect(wrapperDrift(pageSchema(schema), catalog)).toEqual([]);
    },
  );

  test("each failure the page explains is a failure of the catalog, with the members the page reads", () => {
    const variants = list(pageSchema(FailureSchema)["anyOf"]).map(json);
    const problems = variants.flatMap((variant) => {
      const tag = literals(json(json(variant["properties"])["_tag"]))?.[0];
      return drift(variant, failureOf(String(tag)), String(tag));
    });
    expect(problems).toEqual([]);
  });

  test("the comparison finds a member the catalog does not have, and a literal it does not know", () => {
    const renamed = Schema.Struct({ ...IssueSummarySchema.fields, revisions: Schema.Int });
    const widened = Schema.Struct({
      ...IssueSummarySchema.fields,
      category: Schema.Literals([
        "backlog",
        "unstarted",
        "started",
        "completed",
        "canceled",
        "frozen",
      ]),
    });
    expect(drift(pageSchema(renamed), { $ref: "#/$defs/IssueSummary" }, "")).toContain(
      ".revisions: the catalog has no such member",
    );
    expect(drift(pageSchema(widened), { $ref: "#/$defs/IssueSummary" }, "")).toHaveLength(1);
  });
});
