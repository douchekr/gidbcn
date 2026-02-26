use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio::sync::{mpsc, oneshot};

use crate::config::{Config, KisApiConfig};
use crate::models::messages::ApiRequest;
use crate::models::portfolio::Market;

use super::{auth, bond, domestic, exchange, overseas, stock_info};

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
            .connect_timeout(std::time::Duration::from_secs(5))  // TCP+TLS 연결 수립
            .timeout(std::time::Duration::from_secs(15))          // 전체 요청 (연결~응답 수신)
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
        Ok(headers)
    }

    async fn refresh_token(&mut self, full_config: &mut Config) {
        tracing::info!("Refreshing access token...");
        match auth::issue_token(&self.client, &self.config).await {
            Ok(token_info) => {
                tracing::info!("Token refreshed, expires at {}", token_info.expires_at);
                self.config.token = Some(token_info.clone());
                full_config.kis_api.token = Some(token_info);
                if let Err(e) = full_config.save(storage::CONFIG_PATH) {
                    tracing::error!("Failed to save config after token refresh: {e}");
                }
            }
            Err(e) => {
                tracing::error!("Token refresh failed: {e}");
            }
        }
    }
}

/// API Actor 메인 루프
pub async fn run_api_actor(mut rx: mpsc::Receiver<ApiRequest>, full_config: Config) {
    let mut ctx = ActorContext::new(full_config.kis_api.clone());
    let mut full_config = full_config;

    // 시작 시 토큰 확인
    if auth::token_needs_refresh(&ctx.config.token) {
        ctx.refresh_token(&mut full_config).await;
    }

    while let Some(req) = rx.recv().await {
        // 토큰 갱신 체크
        if auth::token_needs_refresh(&ctx.config.token) {
            ctx.refresh_token(&mut full_config).await;
        }

        match req {
            ApiRequest::GetDomesticPrice { symbol, respond_to } => {
                ctx.rate_limit().await;
                let result = domestic::get_price(&ctx, &symbol).await;
                let _ = respond_to.send(result);
            }
            ApiRequest::GetOverseasPrice {
                exchange: exch,
                symbol,
                respond_to,
            } => {
                ctx.rate_limit().await;
                let result = overseas::get_price(&ctx, &exch, &symbol).await;
                let _ = respond_to.send(result);
            }
            ApiRequest::GetBondPrice { isin, respond_to } => {
                ctx.rate_limit().await;
                let result = bond::get_price(&ctx, &isin).await;
                let _ = respond_to.send(result);
            }
            ApiRequest::GetExchangeRate { respond_to } => {
                ctx.rate_limit().await;
                let result = match exchange::get_usd_krw(&ctx).await {
                    Ok(rate) => {
                        // 성공 시 config.json에 캐싱
                        let kst = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
                        full_config.exchange_rate.usd_krw = rate;
                        full_config.exchange_rate.updated_at =
                            Some(chrono::Utc::now().with_timezone(&kst));
                        if let Err(e) = full_config.save(storage::CONFIG_PATH) {
                            tracing::warn!("Failed to save exchange rate to config: {e}");
                        }
                        tracing::info!("USD/KRW = {rate} (saved to config)");
                        Ok(rate)
                    }
                    Err(e) => {
                        // 실패 시 캐시된 환율 사용
                        if full_config.exchange_rate.updated_at.is_some() {
                            let cached = full_config.exchange_rate.usd_krw;
                            tracing::warn!("Exchange rate API failed, using cached {cached}: {e}");
                            Ok(cached)
                        } else {
                            Err(e)
                        }
                    }
                };
                let _ = respond_to.send(result);
            }
            ApiRequest::GetStockName { prdt_type_cd, pdno, respond_to } => {
                ctx.rate_limit().await;
                let result = stock_info::get_stock_name(&ctx, &prdt_type_cd, &pdno).await;
                let _ = respond_to.send(result);
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
        }
    }
}
