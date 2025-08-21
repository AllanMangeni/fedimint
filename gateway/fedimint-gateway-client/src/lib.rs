use bitcoin::address::NetworkUnchecked;
use bitcoin::{Address, Txid};
use fedimint_core::util::SafeUrl;
use fedimint_gateway_common::{
    ADDRESS_ENDPOINT, ADDRESS_RECHECK_ENDPOINT, BACKUP_ENDPOINT, BackupPayload,
    CLOSE_CHANNELS_WITH_PEER_ENDPOINT, CONFIGURATION_ENDPOINT, CONNECT_FED_ENDPOINT,
    CREATE_BOLT11_INVOICE_FOR_OPERATOR_ENDPOINT, CREATE_BOLT12_OFFER_FOR_OPERATOR_ENDPOINT,
    ChannelInfo, CloseChannelsWithPeerRequest, CloseChannelsWithPeerResponse, ConfigPayload,
    ConnectFedPayload, CreateInvoiceForOperatorPayload, CreateOfferPayload, CreateOfferResponse,
    DepositAddressPayload, DepositAddressRecheckPayload, FederationInfo, GATEWAY_INFO_ENDPOINT,
    GATEWAY_INFO_POST_ENDPOINT, GET_BALANCES_ENDPOINT, GET_INVOICE_ENDPOINT,
    GET_LN_ONCHAIN_ADDRESS_ENDPOINT, GatewayBalances, GatewayFedConfig, GatewayInfo,
    GetInvoiceRequest, GetInvoiceResponse, LEAVE_FED_ENDPOINT, LIST_CHANNELS_ENDPOINT,
    LIST_TRANSACTIONS_ENDPOINT, LeaveFedPayload, ListTransactionsPayload, ListTransactionsResponse,
    MNEMONIC_ENDPOINT, MnemonicResponse, OPEN_CHANNEL_ENDPOINT, OpenChannelRequest,
    PAY_INVOICE_FOR_OPERATOR_ENDPOINT, PAY_OFFER_FOR_OPERATOR_ENDPOINT, PAYMENT_LOG_ENDPOINT,
    PAYMENT_SUMMARY_ENDPOINT, PayInvoiceForOperatorPayload, PayOfferPayload, PayOfferResponse,
    PaymentLogPayload, PaymentLogResponse, PaymentSummaryPayload, PaymentSummaryResponse,
    RECEIVE_ECASH_ENDPOINT, ReceiveEcashPayload, ReceiveEcashResponse, SEND_ONCHAIN_ENDPOINT,
    SET_FEES_ENDPOINT, SPEND_ECASH_ENDPOINT, STOP_ENDPOINT, SendOnchainRequest, SetFeesPayload,
    SpendEcashPayload, SpendEcashResponse, WITHDRAW_ENDPOINT, WithdrawPayload, WithdrawResponse,
};
use lightning_invoice::Bolt11Invoice;
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

pub struct GatewayRpcClient {
    /// Base URL to gateway web server
    /// This should include an applicable API version, e.g. http://localhost:8080/v1
    base_url: SafeUrl,
    /// A request client
    client: reqwest::Client,
    /// Optional gateway password
    password: Option<String>,
}

impl GatewayRpcClient {
    pub fn new(versioned_api: SafeUrl, password: Option<String>) -> Self {
        Self {
            base_url: versioned_api,
            client: reqwest::Client::new(),
            password,
        }
    }

    pub fn with_password(&self, password: Option<String>) -> Self {
        GatewayRpcClient::new(self.base_url.clone(), password)
    }

    pub async fn get_info(&self) -> GatewayRpcResult<GatewayInfo> {
        let url = self
            .base_url
            .join(GATEWAY_INFO_ENDPOINT)
            .expect("invalid base url");
        self.call_get(url).await
    }

    // FIXME: deprecated >= 0.3.0
    pub async fn get_info_legacy(&self) -> GatewayRpcResult<GatewayInfo> {
        let url = self
            .base_url
            .join(GATEWAY_INFO_POST_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, ()).await
    }

    pub async fn get_config(&self, payload: ConfigPayload) -> GatewayRpcResult<GatewayFedConfig> {
        let url = self
            .base_url
            .join(CONFIGURATION_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn get_deposit_address(
        &self,
        payload: DepositAddressPayload,
    ) -> GatewayRpcResult<Address<NetworkUnchecked>> {
        let url = self
            .base_url
            .join(ADDRESS_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn withdraw(&self, payload: WithdrawPayload) -> GatewayRpcResult<WithdrawResponse> {
        let url = self
            .base_url
            .join(WITHDRAW_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn connect_federation(
        &self,
        payload: ConnectFedPayload,
    ) -> GatewayRpcResult<FederationInfo> {
        let url = self
            .base_url
            .join(CONNECT_FED_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn leave_federation(
        &self,
        payload: LeaveFedPayload,
    ) -> GatewayRpcResult<FederationInfo> {
        let url = self
            .base_url
            .join(LEAVE_FED_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn backup(&self, payload: BackupPayload) -> GatewayRpcResult<()> {
        let url = self
            .base_url
            .join(BACKUP_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn set_fees(&self, payload: SetFeesPayload) -> GatewayRpcResult<()> {
        let url = self
            .base_url
            .join(SET_FEES_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn create_invoice_for_self(
        &self,
        payload: CreateInvoiceForOperatorPayload,
    ) -> GatewayRpcResult<Bolt11Invoice> {
        let url = self
            .base_url
            .join(CREATE_BOLT11_INVOICE_FOR_OPERATOR_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn pay_invoice(
        &self,
        payload: PayInvoiceForOperatorPayload,
    ) -> GatewayRpcResult<String> {
        let url = self
            .base_url
            .join(PAY_INVOICE_FOR_OPERATOR_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn get_ln_onchain_address(&self) -> GatewayRpcResult<Address<NetworkUnchecked>> {
        let url = self
            .base_url
            .join(GET_LN_ONCHAIN_ADDRESS_ENDPOINT)
            .expect("invalid base url");
        self.call_get(url).await
    }

    pub async fn open_channel(&self, payload: OpenChannelRequest) -> GatewayRpcResult<Txid> {
        let url = self
            .base_url
            .join(OPEN_CHANNEL_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn close_channels_with_peer(
        &self,
        payload: CloseChannelsWithPeerRequest,
    ) -> GatewayRpcResult<CloseChannelsWithPeerResponse> {
        let url = self
            .base_url
            .join(CLOSE_CHANNELS_WITH_PEER_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn list_channels(&self) -> GatewayRpcResult<Vec<ChannelInfo>> {
        let url = self
            .base_url
            .join(LIST_CHANNELS_ENDPOINT)
            .expect("invalid base url");
        self.call_get(url).await
    }

    pub async fn send_onchain(&self, payload: SendOnchainRequest) -> GatewayRpcResult<Txid> {
        let url = self
            .base_url
            .join(SEND_ONCHAIN_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn recheck_address(
        &self,
        payload: DepositAddressRecheckPayload,
    ) -> GatewayRpcResult<serde_json::Value> {
        let url = self
            .base_url
            .join(ADDRESS_RECHECK_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn spend_ecash(
        &self,
        payload: SpendEcashPayload,
    ) -> GatewayRpcResult<SpendEcashResponse> {
        let url = self
            .base_url
            .join(SPEND_ECASH_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn receive_ecash(
        &self,
        payload: ReceiveEcashPayload,
    ) -> GatewayRpcResult<ReceiveEcashResponse> {
        let url = self
            .base_url
            .join(RECEIVE_ECASH_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn get_balances(&self) -> GatewayRpcResult<GatewayBalances> {
        let url = self
            .base_url
            .join(GET_BALANCES_ENDPOINT)
            .expect("invalid base url");
        self.call_get(url).await
    }

    pub async fn get_mnemonic(&self) -> GatewayRpcResult<MnemonicResponse> {
        let url = self
            .base_url
            .join(MNEMONIC_ENDPOINT)
            .expect("invalid base url");
        self.call_get(url).await
    }

    pub async fn stop(&self) -> GatewayRpcResult<()> {
        let url = self.base_url.join(STOP_ENDPOINT).expect("invalid base url");
        self.call_get(url).await
    }

    pub async fn payment_log(
        &self,
        payload: PaymentLogPayload,
    ) -> GatewayRpcResult<PaymentLogResponse> {
        let url = self
            .base_url
            .join(PAYMENT_LOG_ENDPOINT)
            .expect("Invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn payment_summary(
        &self,
        payload: PaymentSummaryPayload,
    ) -> GatewayRpcResult<PaymentSummaryResponse> {
        let url = self
            .base_url
            .join(PAYMENT_SUMMARY_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn get_invoice(
        &self,
        payload: GetInvoiceRequest,
    ) -> GatewayRpcResult<Option<GetInvoiceResponse>> {
        let url = self
            .base_url
            .join(GET_INVOICE_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn list_transactions(
        &self,
        payload: ListTransactionsPayload,
    ) -> GatewayRpcResult<ListTransactionsResponse> {
        let url = self
            .base_url
            .join(LIST_TRANSACTIONS_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn create_offer(
        &self,
        payload: CreateOfferPayload,
    ) -> GatewayRpcResult<CreateOfferResponse> {
        let url = self
            .base_url
            .join(CREATE_BOLT12_OFFER_FOR_OPERATOR_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    pub async fn pay_offer(&self, payload: PayOfferPayload) -> GatewayRpcResult<PayOfferResponse> {
        let url = self
            .base_url
            .join(PAY_OFFER_FOR_OPERATOR_ENDPOINT)
            .expect("invalid base url");
        self.call_post(url, payload).await
    }

    async fn call<P: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        url: SafeUrl,
        payload: Option<P>,
    ) -> Result<T, GatewayRpcError> {
        let mut builder = self.client.request(method, url.clone().to_unsafe());
        if let Some(password) = self.password.clone() {
            builder = builder.bearer_auth(password);
        }
        if let Some(payload) = payload {
            builder = builder
                .json(&payload)
                .header(reqwest::header::CONTENT_TYPE, "application/json");
        }

        let response = builder.send().await?;

        match response.status() {
            StatusCode::OK => Ok(response.json::<T>().await?),
            status => Err(GatewayRpcError::BadStatus(status)),
        }
    }

    async fn call_get<T: DeserializeOwned>(&self, url: SafeUrl) -> Result<T, GatewayRpcError> {
        self.call(Method::GET, url, None::<()>).await
    }

    async fn call_post<P: Serialize, T: DeserializeOwned>(
        &self,
        url: SafeUrl,
        payload: P,
    ) -> Result<T, GatewayRpcError> {
        self.call(Method::POST, url, Some(payload)).await
    }
}

pub type GatewayRpcResult<T> = Result<T, GatewayRpcError>;

#[derive(Error, Debug)]
pub enum GatewayRpcError {
    #[error("Bad status returned {0}")]
    BadStatus(StatusCode),
    #[error(transparent)]
    RequestError(#[from] reqwest::Error),
}
