//! 批量检索的异步任务与请求上下文传播。

use super::arguments::{node_id_to_js, optional_bool, optional_number, parse_search_usize};
use super::database::{DbBackend, TriviumDB};
use super::errors::to_napi_error;
use super::options::parse_id_search_config;
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
    ids_only: bool,
}

pub enum BatchSearchOutput {
    Hits(Vec<Vec<crate::node::SearchHit>>),
    Ids(Vec<Vec<u64>>),
}

impl BatchSearchTask {
    fn run<T: crate::VectorType>(
        &self,
        handle: &crate::database::SearchHandle<T>,
        rows: Vec<Vec<T>>,
    ) -> crate::Result<(BatchSearchOutput, crate::resources::RequestWorkspace)> {
        if self.ids_only {
            handle
                .search_ids_batch_with_workspace(&rows, &self.search_config, &self.batch_config)
                .map(|(ids, workspace)| (BatchSearchOutput::Ids(ids), workspace))
        } else {
            handle
                .search_batch_with_workspace(&rows, &self.search_config, &self.batch_config)
                .map(|(hits, workspace)| (BatchSearchOutput::Hits(hits), workspace))
        }
    }
}

impl napi::Task for BatchSearchTask {
    type Output = crate::Result<BatchSearchOutput>;
    type JsValue = Either<Vec<Vec<JsSearchHit>>, Vec<Vec<f64>>>;

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
            SearchBackendHandle::F32(db) => queries.into_f32().and_then(|rows| self.run(db, rows)),
            SearchBackendHandle::F16(db) => queries.into_half().and_then(|rows| self.run(db, rows)),
            SearchBackendHandle::U64(db) => queries
                .into_u64("queryVectors")
                .and_then(|rows| self.run(db, rows)),
        };
        Ok(output.map(|(hits, workspace)| {
            self.response_workspace = Some(workspace);
            hits
        }))
    }

    fn resolve(&mut self, env: napi::Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        let _response_workspace = self.response_workspace.take();
        match output.map_err(|error| to_napi_error(env, error))? {
            BatchSearchOutput::Hits(hits) => hits
                .into_iter()
                .map(|hits| {
                    hits.into_iter()
                        .map(|value| search_hit_to_js(env, value))
                        .collect::<napi::Result<_>>()
                })
                .collect::<napi::Result<_>>()
                .map(Either::A),
            BatchSearchOutput::Ids(ids) => ids
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|id| node_id_to_js(env, id))
                        .collect::<napi::Result<_>>()
                })
                .collect::<napi::Result<_>>()
                .map(Either::B),
        }
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
            ids_only: false,
        }))
    }

    #[napi]
    pub fn search_ids_batch(
        &self,
        env: Env,
        query_vectors: JsUnknown,
        config: Option<JsUnknown>,
        parallelism: Option<Either<f64, JsUnknown>>,
    ) -> napi::Result<napi::bindgen_prelude::AsyncTask<BatchSearchTask>> {
        let queries = Batch::read(env, query_vectors, self.dim() as usize, &self.dtype, false)?;
        let search_config = parse_id_search_config(env, config)?;
        let parallelism = parse_search_usize(env, parallelism, 0.0, "parallelism", false)?;
        let backend = match &self.inner {
            DbBackend::F32(db) => SearchBackendHandle::F32(db.search_handle()),
            DbBackend::F16(db) => SearchBackendHandle::F16(db.search_handle()),
            DbBackend::U64(db) => SearchBackendHandle::U64(db.search_handle()),
        };
        Ok(napi::bindgen_prelude::AsyncTask::new(BatchSearchTask {
            context: crate::request::current(),
            backend,
            queries,
            response_workspace: None,
            search_config,
            batch_config: crate::database::BatchSearchConfig { parallelism },
            ids_only: true,
        }))
    }
}
