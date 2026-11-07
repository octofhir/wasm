//! FHIR in the browser: the validator, FHIRPath, SQL on FHIR, FML and UCUM of OctoFHIR, in
//! WebAssembly.
//!
//! Everything goes in and comes out as plain JavaScript values. Failures are rejected with an
//! `Error`.

use std::collections::HashMap;
use std::sync::Arc;

use fml_check::{CoreDefinitions, Release};
use fml_engine::{Engine, FhirPathExpressions};
use octofhir_fhirpath::evaluator::create_function_registry;
use octofhir_fhirpath::{Collection, EvaluationContext, FhirPathEngine};
use octofhir_fhirschema::{
    FhirSchema, FhirSchemaModelProvider, FhirValidator, FhirVersion, InMemorySchemaProvider,
    ModelFhirVersion, StructureDefinition, get_schemas, translate,
};
use octofhir_sof::ViewDefinition;
use serde::Serialize;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

fn failure(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}

fn from_js(value: JsValue) -> Result<Value, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(failure)
}

fn to_js(value: &impl Serialize) -> Result<JsValue, JsError> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(failure)
}

fn versions(version: &str) -> Result<(FhirVersion, ModelFhirVersion), JsError> {
    let found = match version {
        "R4" => (FhirVersion::R4, ModelFhirVersion::R4),
        "R4B" => (FhirVersion::R4B, ModelFhirVersion::R4B),
        "R5" => (FhirVersion::R5, ModelFhirVersion::R5),
        "R6" => (FhirVersion::R6, ModelFhirVersion::R6),
        other => return Err(failure(format!("Unknown FHIR version {other}"))),
    };
    // A build for one version holds no definitions of the others.
    if get_schemas(found.0).is_empty() {
        return Err(failure(format!(
            "This build has no {version}: use @octofhir/wasm, which has all four"
        )));
    }
    Ok(found)
}

/// The core of a FHIR version and the profiles given to it.
#[wasm_bindgen]
pub struct Fhir {
    engine: Arc<FhirPathEngine>,
    validator: FhirValidator,
    model: Arc<FhirSchemaModelProvider>,
}

#[wasm_bindgen]
impl Fhir {
    /// `version` is `R4`, `R4B`, `R5` or `R6`. `profiles` is an array of StructureDefinitions.
    pub async fn create(version: String, profiles: JsValue) -> Result<Fhir, JsError> {
        let (version, model_version) = versions(&version)?;
        let mut schemas: HashMap<String, FhirSchema> = get_schemas(version).clone();
        if !profiles.is_undefined() && !profiles.is_null() {
            let definitions: Vec<StructureDefinition> =
                serde_wasm_bindgen::from_value(profiles).map_err(failure)?;
            for definition in definitions {
                let schema = translate(definition, None).map_err(failure)?;
                schemas.insert(schema.url.clone(), schema);
            }
        }
        let shared: HashMap<String, Arc<FhirSchema>> = schemas
            .iter()
            .map(|(name, schema)| (name.clone(), Arc::new(schema.clone())))
            .collect();
        let model = Arc::new(FhirSchemaModelProvider::new(schemas, model_version));
        let engine = Arc::new(
            FhirPathEngine::new(Arc::new(create_function_registry()), model.clone())
                .await
                .map_err(failure)?,
        );
        let validator = FhirValidator::new_with_fhirpath(
            Arc::new(InMemorySchemaProvider::from_map(shared)),
            engine.clone(),
        );
        Ok(Self {
            engine,
            validator,
            model,
        })
    }

    /// Evaluate a FHIRPath expression, on a resource when given. The result is an array.
    pub async fn evaluate(
        &self,
        expression: String,
        resource: JsValue,
    ) -> Result<JsValue, JsError> {
        let input = if resource.is_undefined() || resource.is_null() {
            Collection::empty()
        } else {
            Collection::from_json_resource(from_js(resource)?, Some(self.model.clone()))
                .await
                .map_err(failure)?
        };
        let context = EvaluationContext::new(input, self.model.clone(), None, None, None);
        let result = self
            .engine
            .evaluate(&expression, &context)
            .await
            .map_err(failure)?;
        let values: Vec<Value> = result
            .value
            .iter()
            .map(|item| item.to_json_value())
            .collect();
        to_js(&values)
    }

    /// Validate a resource, against a profile too when its canonical URL is given. Code bindings
    /// are not checked: that takes a terminology server.
    pub async fn validate(
        &self,
        resource: JsValue,
        profile: Option<String>,
    ) -> Result<JsValue, JsError> {
        let resource = from_js(resource)?;
        let kind = resource["resourceType"]
            .as_str()
            .ok_or_else(|| failure("The resource has no resourceType"))?;
        let mut schemas = vec![kind.to_owned()];
        schemas.extend(profile);
        let result = self.validator.validate(&resource, schemas).await;
        to_js(&result)
    }
}

/// Run a ViewDefinition over an array of resources: `{columns, rows}`.
#[wasm_bindgen(js_name = runView)]
pub async fn run_view(view: JsValue, resources: JsValue) -> Result<JsValue, JsError> {
    let view = ViewDefinition::from_json(&from_js(view)?).map_err(failure)?;
    let resources = from_js(resources)?;
    let resources = resources
        .as_array()
        .ok_or_else(|| failure("resources is an array"))?;
    let result = octofhir_sof::execute(&view, resources)
        .await
        .map_err(failure)?;
    let columns: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
    let rows: Vec<Value> = result
        .data
        .iter()
        .map(|row| {
            columns
                .iter()
                .zip(row)
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect()
        })
        .collect();
    to_js(&json!({"columns": columns, "rows": rows}))
}

fn release(version: &str) -> Result<Release, JsError> {
    match version {
        "R4" => Ok(Release::R4),
        "R4B" => Ok(Release::R4B),
        "R5" => Ok(Release::R5),
        "R6" => Ok(Release::R6),
        other => Err(failure(format!("Unknown FHIR version {other}"))),
    }
}

/// Check a FHIR Mapping Language map: its syntax, and its paths and types against `version`.
/// An array of `{severity, message, start, end}`, offsets in bytes.
#[wasm_bindgen(js_name = fmlCheck)]
pub fn fml_check(source: &str, version: &str) -> Result<JsValue, JsError> {
    let definitions = CoreDefinitions::new(release(version)?);
    let parsed = fml_syntax::parse(source);
    let found: Vec<Value> = parsed
        .diagnostics()
        .iter()
        .chain(&fml_check::check(&parsed.tree(), &definitions))
        .map(|diagnostic| {
            json!({
                "severity": format!("{:?}", diagnostic.severity).to_lowercase(),
                "message": diagnostic.message,
                "start": u32::from(diagnostic.range.start()),
                "end": u32::from(diagnostic.range.end()),
            })
        })
        .collect();
    to_js(&found)
}

/// Run a FHIR Mapping Language map over a resource: `{target, log, warnings, produced}`.
#[wasm_bindgen(js_name = fmlTransform)]
pub async fn fml_transform(
    source: String,
    input: JsValue,
    version: String,
) -> Result<JsValue, JsError> {
    let release = release(&version)?;
    let compiled = fml_model::compile(&source);
    if let Some(error) = compiled.diagnostics.first() {
        return Err(failure(&error.message));
    }
    let definitions = CoreDefinitions::new(release);
    let expressions = FhirPathExpressions::new(release).await.map_err(failure)?;
    let output = Engine::new(&definitions, expressions)
        .transform(&compiled.map, &from_js(input)?)
        .await
        .map_err(failure)?;
    to_js(&json!({
        "target": output.target,
        "log": output.log,
        "warnings": output.warnings,
        "produced": output.produced,
    }))
}

/// Format a FHIR Mapping Language map. Rejected when the map has syntax errors.
#[wasm_bindgen(js_name = fmlFormat)]
pub fn fml_format(source: &str) -> Result<String, JsError> {
    fml_syntax::format(source, &fml_syntax::FormatOptions::default()).map_err(|error| match error {
        fml_syntax::FormatError::Syntax(diagnostics) => failure(
            diagnostics
                .iter()
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ),
        other => failure(format!("{other:?}")),
    })
}

/// Compile a FHIR Mapping Language map to a StructureMap: the R4 form for `R4` and `R4B`, the R5
/// form otherwise.
#[wasm_bindgen(js_name = fmlCompile)]
pub fn fml_compile(source: &str, version: &str) -> Result<JsValue, JsError> {
    let release = release(version)?;
    let compiled = fml_model::compile(source);
    if let Some(error) = compiled
        .diagnostics
        .iter()
        .find(|d| d.severity == fml_syntax::Severity::Error)
    {
        return Err(failure(&error.message));
    }
    let map = match release {
        Release::R4 | Release::R4B => fml_model::to_r4(&compiled.map).json,
        _ => serde_json::to_value(&compiled.map).map_err(failure)?,
    };
    to_js(&map)
}

/// Print a StructureMap (R4 or R5) as FML.
#[wasm_bindgen(js_name = fmlPrint)]
pub fn fml_print(map: JsValue) -> Result<String, JsError> {
    let map = fml_model::from_json(from_js(map)?).map_err(failure)?;
    Ok(fml_model::print(&map))
}

/// The FML language server, for an editor in the page: completion, hover, go to definition,
/// references, rename, signature help, outline, formatting and semantic tokens, over the core
/// definitions of a FHIR version. Messages are Language Server Protocol JSON-RPC objects.
#[wasm_bindgen]
pub struct FmlLanguageServer {
    server: fml_lsp::Server,
}

#[wasm_bindgen]
impl FmlLanguageServer {
    #[wasm_bindgen(constructor)]
    pub fn new(version: &str) -> Result<FmlLanguageServer, JsError> {
        let definitions = Arc::new(CoreDefinitions::new(release(version)?));
        Ok(Self {
            server: fml_lsp::Server::new(definitions),
        })
    }

    /// Handle a message from the editor; the replies to send back, as an array: the response to
    /// a request, and notifications such as `textDocument/publishDiagnostics`.
    pub fn handle(&mut self, message: JsValue) -> Result<JsValue, JsError> {
        to_js(&self.server.handle(&from_js(message)?))
    }
}

/// Whether a UCUM expression is valid.
#[wasm_bindgen(js_name = ucumValidate)]
pub fn ucum_validate(expression: &str) -> bool {
    octofhir_ucum::validate(expression).is_ok()
}

/// Whether two UCUM units measure the same property.
#[wasm_bindgen(js_name = ucumComparable)]
pub fn ucum_comparable(left: &str, right: &str) -> Result<bool, JsError> {
    octofhir_ucum::is_comparable(left, right).map_err(failure)
}
