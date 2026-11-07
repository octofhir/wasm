// Runs the Node build: wasm-pack build --dev --target nodejs --out-dir pkg-node
import { createRequire } from "node:module";
import assert from "node:assert/strict";

const require = createRequire(import.meta.url);
const {
  Fhir, fmlCheck, fmlTransform, fmlFormat, fmlCompile, fmlPrint, FmlLanguageServer,
  runView, ucumValidate, ucumComparable,
} = require("../pkg-node/octofhir_wasm.js");

const patient = {
  resourceType: "Patient",
  id: "p",
  gender: "female",
  name: [{ family: "Doe", given: ["Ann"] }],
};

const profile = {
  resourceType: "StructureDefinition",
  url: "http://example.org/StructureDefinition/named",
  name: "Named",
  status: "active",
  fhirVersion: "4.0.1",
  kind: "resource",
  abstract: false,
  type: "Patient",
  baseDefinition: "http://hl7.org/fhir/StructureDefinition/Patient",
  derivation: "constraint",
  differential: {
    element: [
      { id: "Patient", path: "Patient" },
      { id: "Patient.birthDate", path: "Patient.birthDate", min: 1 },
    ],
  },
};

assert.equal(ucumValidate("mg/dL"), true);
assert.equal(ucumValidate("nonsense["), false);
assert.equal(ucumComparable("mg", "kg"), true);
console.log("ucum ok");

const view = {
  resourceType: "ViewDefinition",
  status: "active",
  resource: "Patient",
  select: [{ column: [{ name: "id", path: "id" }, { name: "gender", path: "gender" }] }],
};
const table = await runView(view, [patient]);
assert.deepEqual(table.rows, [{ id: "p", gender: "female" }]);
console.log("view ok");

const started = performance.now();
const fhir = await Fhir.create("R4", [profile]);
console.log("create", Math.round(performance.now() - started), "ms");

assert.deepEqual(await fhir.evaluate("name.given.first()", patient), ["Ann"]);
assert.deepEqual(await fhir.evaluate("1 + 2"), [3]);
await assert.rejects(() => fhir.evaluate("1 +"));
console.log("fhirpath ok");

const valid = await fhir.validate(patient);
assert.equal(valid.valid, true, JSON.stringify(valid));
const broken = await fhir.validate({ resourceType: "Patient", nonsense: 1 });
assert.equal(broken.valid, false);
const missing = await fhir.validate(patient, profile.url);
assert.equal(missing.valid, false, JSON.stringify(missing));
console.log("validate ok", missing.errors[0].message);

const map = `map "http://example.org/StructureMap/copy" = "copy"
uses "http://hl7.org/fhir/StructureDefinition/Patient" as source
uses "http://hl7.org/fhir/StructureDefinition/Patient" as target
group copy(source src : Patient, target tgt : Patient) {
  src.gender as g -> tgt.gender = g;
  src.id -> tgt.id = 'copied' "id";
}
`;
assert.deepEqual(fmlCheck(map, "R4"), []);
const typo = fmlCheck(map.replace("tgt.gender", "tgt.gendr"), "R4");
assert.equal(typo.length, 1, JSON.stringify(typo));
const output = await fmlTransform(map, patient, "R4");
assert.equal(output.target.gender, "female", JSON.stringify(output));
assert.equal(output.target.id, "copied");
console.log("fml ok", typo[0].message);

assert.ok(fmlFormat(map.replace("src.gender as g", "src.gender   as g")).includes("src.gender as g"));
const structureMap = fmlCompile(map, "R4");
assert.equal(structureMap.resourceType, "StructureMap");
assert.ok(fmlPrint(structureMap).includes("group copy("));
const lsp = new FmlLanguageServer("R4");
lsp.handle({ jsonrpc: "2.0", id: 1, method: "initialize", params: { capabilities: {} } });
const opened = lsp.handle({
  jsonrpc: "2.0", method: "textDocument/didOpen",
  params: { textDocument: { uri: "file:///m.fml", languageId: "fml", version: 1, text: map } },
});
assert.deepEqual(opened[0].params.diagnostics, []);
const [completion] = lsp.handle({
  jsonrpc: "2.0", id: 2, method: "textDocument/completion",
  params: { textDocument: { uri: "file:///m.fml" }, position: { line: 4, character: 6 } },
});
assert.ok(completion.result.items.some((item) => item.label === "birthDate"), JSON.stringify(completion));
console.log("fml tools ok");

// A build for one version (OCTOFHIR_FHIR_VERSIONS=R4) says so for the others.
if (process.env.OCTOFHIR_FHIR_VERSIONS === "R4") {
  await assert.rejects(Fhir.create("R5", []), /This build has no R5/);
  console.log("one version ok");
}
