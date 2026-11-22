//! 批量检索的异步任务与请求上下文传播。

use super::arguments::{optional_bool, optional_number, parse_search_usize};
use super::database::{DbBackend, TriviumDB};
use super::errors::to_napi_error;
use super::types::{JsSearchHit, search_hit_to_js};
use super::vectors::Batch;
use napi::bindgen_prelude::Either;
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[derive(Clone)]
enum SearchBackendHandle {
    F32(crate::database::SearchHandle<f32>),
    F16(crate::database::SearchHandle<half::f16>),
    U64(crate::database::SearchHandle<u64>),
}

pub struct BatchSearchTask {
    context: Option<crate::sdk::RequestContext>,
    backend: SearchBackendHandle,
    queries: Batch,
    response_workspace: Option<crate::resources::RequestWorkspace>,
    search_config: crate::database::SearchConfig,
    batch_config: crate::database::BatchSearchConfig,
}

impl napi::Task for BatchSearchTask {
    type Output = crate::Result<Vec<Vec<crate::node::SearchHit>>>;
    type JsValue = Vec<Vec<JsSearchHit>>;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        let _scope = match self
            .context
            .as_ref()
            .map(crate::sdk::RequestContext::enter)
            .transpose()
        {
            Ok(scope) => scope,
            Err(error) => return Ok(Err(error)),
        };
        if let Err(error) = crate::request::check_current() {
            return Ok(Err(error));
        }
        let queries = std::mem::replace(&mut self.queries, Batch::Numbers(Vec::new()));
        let output = match &self.backend {
            SearchBackendHandle::F32(db) => queries.into_f32().and_then(|rows| {
                db.search_batch_with_workspace(&rows, &self.search_config, &self.batch_config)
            }),
            SearchBackendHandle::F16(db) => queries.into_half().and_then(|rows| {
                db.search_batch_with_workspace(&rows, &self.search_config, &self.batch_config)
            }),
            SearchBackendHandle::U64(db) => queries.into_u64("queryVectors").and_then(|rows| {
                db.search_batch_with_workspace(&rows, &self.search_config, &self.batch_config)
            }),
        };
        Ok(output.map(|(hits, workspace)| {
            self.response_workspace = Some(workspace);
            hits
        }))
    }

    fn resolve(&mut self, env: napi::Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        let _response_workspace = self.response_workspace.take();
        output
            .map_err(|error| to_napi_error(env, error))?
            .into_iter()
            .map(|hits| {
                hits.into_iter()
                    .map(|value| search_hit_to_js(env, value))
                    .collect::<napi::Result<_>>()
            })
            .collect::<napi::Result<_>>()
    }
}

#[napi]
impl TriviumDB {
    #[napi]
    pub fn search_batch(
        &self,
        env: Env,
        query_vectors: JsUnknown,
        top_k: Option<Either<f64, JsUnknown>>,
        parallelism: Option<Either<f64, JsUnknown>>,
        min_score: Option<JsUnknown>,
        exact: Option<JsUnknown>,
    ) -> napi::Result<napi::bindgen_prelude::AsyncTask<BatchSearchTask>> {
        let query_vectors =
            Batch::read(env, query_vectors, self.dim() as usize, &self.dtype, false)?;
        let top_k = parse_search_usize(env, top_k, 5.0, "topK", true)?;
        let parallelism = parse_search_usize(env, parallelism, 0.0, "parallelism", false)?;
        let backend = match &self.inner {
            DbBackend::F32(db) => SearchBackendHandle::F32(db.search_handle()),
            DbBackend::F16(db) => SearchBackendHandle::F16(db.search_handle()),
            DbBackend::U64(db) => SearchBackendHandle::U64(db.search_handle()),
        };
        Ok(napi::bindgen_prelude::AsyncTask::new(BatchSearchTask {
            context: crate::request::current(),
            backend,
            queries: query_vectors,
            response_workspace: None,
            search_config: crate::database::SearchConfig {
                top_k,
                expand_depth: 0,
                min_score: optional_number(env, min_score, 0.0, "minScore")? as f32,
                enable_advanced_pipeline: false,
                ..Default::default()
            }
            .with_exact(optional_bool(env, exact, false, "exact")?),
            batch_config: crate::database::BatchSearchConfig { parallelism },
        }))
    }
}
