use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio::sync::{mpsc, oneshot};

use crate::config::KisApiConfig;
use crate::models::messages::ApiRequest;
use crate::models::portfolio::Market;

use super::{auth, bond, domestic, overseas, stock_info};

use crate::storage;

/// Actor 내부 상태 — 외부에서 접근 불가, actor loop만 소유
pub struct ActorContext {
    pub client: reqwest::Client,
    pub config: KisApiConfig,
    last_request: std::time::Instant,
}

impl ActorContext {
    fn new(config: KisApiConfig) -> Self {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(std::time::Duration::from_secs(5))   // TCP+TLS 연결 수립
            .timeout(std::time::Duration::from_secs(15))           // 전체 요청 (연결~응답 수신)
            .pool_idle_timeout(std::time::Duration::from_secs(55)) // stale connection 방지: 서버 idle timeout(~60s)보다 짧게
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            config,
            last_request: std::time::Instant::now()
                - std::time::Duration::from_millis(50),
        }
    }

    /// 초당 20회 제한 (50ms 간격)
    async fn rate_limit(&mut self) {
        let elapsed = self.last_request.elapsed();
        let min_interval = std::time::Duration::from_millis(50);
        if elapsed < min_interval {
            tokio::time::sleep(min_interval - elapsed).await;
        }
        self.last_request = std::time::Instant::now();
    }

    /// 공통 헤더 생성
    pub fn common_headers(&self, tr_id: &str) -> Result<HeaderMap> {
        let token = self
            .config
            .token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token available"))?;

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json; charset=utf-8"));
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", token.access_token))?,
        );
        headers.insert("appkey", HeaderValue::from_str(&self.config.app_key)?);
        headers.insert("appsecret", HeaderValue::from_str(&self.config.app_secret)?);
        headers.insert("tr_id", HeaderValue::from_str(tr_id)?);
        headers.insert("custtype", HeaderValue::from_static("P"));
        Ok(headers)
    }

    async fn refresh_token(&mut self) {
        tracing::info!("Refreshing access token...");
        match auth::issue_token(&self.client, &self.config).await {
            Ok(token_info) => {
                tracing::info!("Token refreshed, expires at {}", token_info.expires_at);
                self.config.token = Some(token_info.clone());
                if let Err(e) = storage::update_config(|c| {
                    c.secrets.set_token(&token_info.access_token, token_info.expires_at);
                }) {
                    tracing::error!("Failed to save config after token refresh: {e}");
                }
            }
            Err(e) => {
                tracing::error!("Token refresh failed: {e}");
            }
        }
    }
}

/// keep-alive 연결이 서버 측에서 종료된 경우 1회 재시도.
/// 타임아웃은 재시도하지 않음.
pub async fn send_with_retry(
    builder: reqwest::RequestBuilder,
) -> reqwest::Result<reqwest::Response> {
    let retry = builder.try_clone();
    match builder.send().await {
        Ok(resp) => Ok(resp),
        Err(e) if !e.is_timeout() && (e.is_request() || e.is_connect()) => {
            tracing::debug!("Stale connection, retrying once: {e}");
            match retry {
                Some(b) => b.send().await,
                None => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

/// in-flight future 완료 시 환율 갱신용
type InFlightFut = Pin<Box<dyn Future<Output = Option<f64>>>>;

/// API Actor 메인 루프 (파이프라인: 발신 후 응답 비동기 수거)
pub async fn run_api_actor(mut rx: mpsc::Receiver<ApiRequest>) {
    let kis_config = storage::with_config(|c| c.to_kis_api_config());
    let mut ctx = ActorContext::new(kis_config);
    let mut usd_krw: f64 = 1350.0;
    let mut in_flight: FuturesUnordered<InFlightFut> = FuturesUnordered::new();

    if auth::token_needs_refresh(&ctx.config.token) {
        ctx.refresh_token().await;
    }

    loop {
        tokio::select! {
            biased;

            Some(rate_update) = in_flight.next(), if !in_flight.is_empty() => {
                if let Some(rate) = rate_update {
                    usd_krw = rate;
                }
            }

            req = rx.recv() => {
                let Some(req) = req else { break };

                if auth::token_needs_refresh(&ctx.config.token) {
                    ctx.refresh_token().await;
                }

                if let ApiRequest::GetExchangeRate { respond_to } = req {
                    let _ = respond_to.send(Ok(usd_krw));
                    continue;
                }

                ctx.rate_limit().await;

                let client = ctx.client.clone();
                let base_url = ctx.config.base_url.clone();

                match req {
                    ApiRequest::GetDomesticPrice { symbol, respond_to } => {
                        let headers = match ctx.common_headers("FHKST01010100") {
                            Ok(h) => h,
                            Err(e) => { let _ = respond_to.send(Err(e)); continue; }
                        };
                        in_flight.push(Box::pin(async move {
                            let result = domestic::get_price(&client, &base_url, headers, &symbol).await;
                            let _ = respond_to.send(result);
                            None
                        }));
                    }
                    ApiRequest::GetOverseasPrice { exchange: exch, symbol, respond_to } => {
                        let headers = match ctx.common_headers("HHDFS76200200") {
                            Ok(h) => h,
                            Err(e) => { let _ = respond_to.send(Err(e)); continue; }
                        };
                        in_flight.push(Box::pin(async move {
                            let result = overseas::get_price(&client, &base_url, headers, &exch, &symbol).await;
                            let rate = result.as_ref().ok().and_then(|(_, r)| *r);
                            let _ = respond_to.send(result.map(|(price, _)| price));
                            rate
                        }));
                    }
                    ApiRequest::GetBondPrice { isin, respond_to } => {
                        let headers = match ctx.common_headers("FHKBJ773400C0") {
                            Ok(h) => h,
                            Err(e) => { let _ = respond_to.send(Err(e)); continue; }
                        };
                        in_flight.push(Box::pin(async move {
                            let result = bond::get_price(&client, &base_url, headers, &isin).await;
                            let _ = respond_to.send(result);
                            None
                        }));
                    }
                    ApiRequest::GetStockName { prdt_type_cd, pdno, respond_to } => {
                        let headers = match ctx.common_headers("CTPF1604R") {
                            Ok(h) => h,
                            Err(e) => { let _ = respond_to.send(Err(e)); continue; }
                        };
                        in_flight.push(Box::pin(async move {
                            let result = stock_info::get_stock_name(&client, &base_url, headers, &prdt_type_cd, &pdno).await;
                            let _ = respond_to.send(result);
                            None
                        }));
                    }
                    ApiRequest::GetOverseasDetail { exchange: exch, symbol, respond_to } => {
                        let headers = match ctx.common_headers("HHDFS76200200") {
                            Ok(h) => h,
                            Err(e) => { let _ = respond_to.send(Err(e)); continue; }
                        };
                        in_flight.push(Box::pin(async move {
                            let result = overseas::get_detail(&client, &base_url, headers, &exch, &symbol).await;
                            let _ = respond_to.send(result);
                            None
                        }));
                    }
                    ApiRequest::GetExchangeRate { .. } => unreachable!(),
                }
            }
        }
    }

    tracing::info!("API Actor shutting down");
}

// --- ApiHandle: Bot Task에서 API Actor에 접근하는 핸들 ---

#[derive(Clone)]
pub struct ApiHandle {
    sender: mpsc::Sender<ApiRequest>,
}

impl ApiHandle {
    pub fn new(sender: mpsc::Sender<ApiRequest>) -> Self {
        Self { sender }
    }

    pub async fn get_domestic_price(&self, symbol: &str) -> Result<crate::models::messages::PriceData> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(ApiRequest::GetDomesticPrice {
                symbol: symbol.to_string(),
                respond_to: tx,
            })
            .await?;
        rx.await?
    }

    pub async fn get_overseas_price(
        &self,
        exchange: &str,
        symbol: &str,
    ) -> Result<crate::models::messages::PriceData> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(ApiRequest::GetOverseasPrice {
                exchange: exchange.to_string(),
                symbol: symbol.to_string(),
                respond_to: tx,
            })
            .await?;
        rx.await?
    }

    pub async fn get_bond_price(&self, isin: &str) -> Result<crate::models::messages::BondData> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(ApiRequest::GetBondPrice {
                isin: isin.to_string(),
                respond_to: tx,
            })
            .await?;
        rx.await?
    }

    pub async fn get_exchange_rate(&self) -> Result<f64> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(ApiRequest::GetExchangeRate { respond_to: tx })
            .await?;
        rx.await?
    }

    /// Market에 따라 상품기본조회(CTPF1604R)로 종목명(prdt_abrv_name) 조회
    pub async fn get_stock_name(&self, market: Market, symbol: &str) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(ApiRequest::GetStockName {
                prdt_type_cd: market.product_type_code().to_string(),
                pdno: symbol.to_string(),
                respond_to: tx,
            })
            .await?;
        rx.await?
    }

    /// 해외주식 상세 (워치리스트 평가용)
    pub async fn get_overseas_detail(
        &self,
        exchange: &str,
        symbol: &str,
    ) -> Result<crate::models::messages::OverseasDetail> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(ApiRequest::GetOverseasDetail {
                exchange: exchange.to_string(),
                symbol: symbol.to_string(),
                respond_to: tx,
            })
            .await?;
        rx.await?
    }

    /// Market에 따라 적절한 현재가 API 호출
    pub async fn get_price_for_market(
        &self,
        market: Market,
        symbol: &str,
    ) -> Result<crate::models::messages::PriceData> {
        match market {
            Market::KRX => self.get_domestic_price(symbol).await,
            Market::NAS | Market::NYS | Market::AMS => {
                self.get_overseas_price(market.exchange_code(), symbol).await
            }
            Market::BOND => {
                let bond = self.get_bond_price(symbol).await?;
                Ok(crate::models::messages::PriceData {
                    name: bond.name,
                    current_price: bond.current_price,
                    change_pct: bond.change_pct,
                })
            }
            Market::CART => anyhow::bail!("CART: 수동 관리 종목"),
        }
    }
}
