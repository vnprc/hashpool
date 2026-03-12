use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone)]
pub struct PrometheusClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct PrometheusResponse<T> {
    status: String,
    data: PrometheusData<T>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PrometheusData<T> {
    #[serde(rename = "resultType")]
    result_type: String,
    result: Vec<T>,
}

#[derive(Debug, Deserialize)]
pub struct PromVectorSample {
    pub metric: HashMap<String, String>,
    pub value: (f64, String),
}

#[derive(Debug, Deserialize)]
pub struct PromMatrixSample {
    pub metric: HashMap<String, String>,
    pub values: Vec<(f64, String)>,
}

impl PrometheusClient {
    pub fn new(
        base_url: String,
        request_timeout_secs: u64,
        pool_idle_timeout_secs: u64,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(request_timeout_secs))
            .pool_idle_timeout(Duration::from_secs(pool_idle_timeout_secs))
            .pool_max_idle_per_host(1)
            .build()?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    pub async fn query_instant(&self, query: &str) -> Result<Vec<PromVectorSample>, String> {
        let url = format!("{}/api/v1/query", self.base_url);
        let response = self
            .client
            .get(&url)
            .query(&[("query", query)])
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !response.status().is_success() {
            return Err(format!("Prometheus query failed: HTTP {}", response.status()));
        }

        let body: PrometheusResponse<PromVectorSample> = response
            .json()
            .await
            .map_err(|e| e.to_string())?;

        if body.status != "success" {
            return Err(body.error.unwrap_or_else(|| "Unknown Prometheus error".to_string()));
        }

        if body.data.result_type != "vector" {
            return Err(format!(
                "Unexpected Prometheus result type: {}",
                body.data.result_type
            ));
        }

        Ok(body.data.result)
    }

    pub async fn query_range(
        &self,
        query: &str,
        start: u64,
        end: u64,
        step_secs: u64,
    ) -> Result<Vec<PromMatrixSample>, String> {
        let url = format!("{}/api/v1/query_range", self.base_url);
        let response = self
            .client
            .get(&url)
            .query(&[
                ("query", query),
                ("start", &start.to_string()),
                ("end", &end.to_string()),
                ("step", &step_secs.to_string()),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !response.status().is_success() {
            return Err(format!("Prometheus query failed: HTTP {}", response.status()));
        }

        let body: PrometheusResponse<PromMatrixSample> = response
            .json()
            .await
            .map_err(|e| e.to_string())?;

        if body.status != "success" {
            return Err(body.error.unwrap_or_else(|| "Unknown Prometheus error".to_string()));
        }

        if body.data.result_type != "matrix" {
            return Err(format!(
                "Unexpected Prometheus result type: {}",
                body.data.result_type
            ));
        }

        Ok(body.data.result)
    }
}
