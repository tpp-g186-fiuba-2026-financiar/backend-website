//! URLs de los modelos deployados en Modal (repo `models`).
//!
//! Todas salen de `MODAL_URL`: un template con `{endpoint}`, que se reemplaza
//! por `<app>-<funcion>` de Modal. Cambiar de workspace es cambiar solo esa
//! variable, por ejemplo:
//! - `https://financiar186--{endpoint}.us-east.modal.run`
//! - `https://matimorales01--{endpoint}.modal.run`
//!
//! Si un modelo tiene su propia variable (`MODAL_LSTM_URL`, ...) seteada, esa
//! tiene prioridad sobre el template: sirve para apuntar uno solo a otro lado
//! (los tests la usan para los mocks).

/// Template que se usa si no hay `MODAL_URL` (el workspace historico).
const DEFAULT_MODAL_URL: &str = "https://matimorales01--{endpoint}.modal.run";

pub struct ModalEndpoint {
    /// Variable que pisa el template solo para este endpoint.
    pub env_var: &'static str,
    /// `<app>-<funcion>` en Modal.
    pub endpoint: &'static str,
}

impl ModalEndpoint {
    pub fn url(&self) -> String {
        if let Ok(url) = std::env::var(self.env_var) {
            return url;
        }
        let template = std::env::var("MODAL_URL").unwrap_or_else(|_| DEFAULT_MODAL_URL.to_string());
        template
            .trim()
            .trim_end_matches('/')
            .replace("{endpoint}", self.endpoint)
    }
}

pub const LSTM: ModalEndpoint = ModalEndpoint {
    env_var: "MODAL_LSTM_URL",
    endpoint: "lstm-trend-model-main",
};
pub const LSTM_PREPARE: ModalEndpoint = ModalEndpoint {
    env_var: "MODAL_LSTM_PREPARE_URL",
    endpoint: "lstm-trend-model-prepare",
};
pub const XGBOOST: ModalEndpoint = ModalEndpoint {
    env_var: "MODAL_XGBOOST_URL",
    endpoint: "xgboost-trend-model-main",
};
pub const XGBOOST_PREPARE: ModalEndpoint = ModalEndpoint {
    env_var: "MODAL_XGBOOST_PREPARE_URL",
    endpoint: "xgboost-trend-model-prepare",
};
pub const TRANSFORMER: ModalEndpoint = ModalEndpoint {
    env_var: "MODAL_TRANSFORMER_URL",
    endpoint: "transformer-trend-model-main",
};
pub const ARIMA: ModalEndpoint = ModalEndpoint {
    env_var: "MODAL_ARIMA_URL",
    endpoint: "arima-model-main",
};
pub const SVM: ModalEndpoint = ModalEndpoint {
    env_var: "MODAL_SVM_URL",
    endpoint: "svm-model-main",
};
pub const GARCH: ModalEndpoint = ModalEndpoint {
    env_var: "MODAL_GARCH_URL",
    endpoint: "garch-model-main",
};
pub const GARCH_ANN: ModalEndpoint = ModalEndpoint {
    env_var: "MODAL_GARCH_ANN_URL",
    endpoint: "garch-ann-model-main",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn builds_each_url_from_the_modal_url_template() {
        let _env_guard = crate::ENV_TEST_LOCK.lock().await;
        std::env::remove_var("MODAL_SVM_URL");
        std::env::set_var(
            "MODAL_URL",
            "https://financiar186--{endpoint}.us-east.modal.run/",
        );

        assert_eq!(
            SVM.url(),
            "https://financiar186--svm-model-main.us-east.modal.run"
        );

        std::env::remove_var("MODAL_URL");
    }

    #[tokio::test]
    async fn model_specific_variable_overrides_the_template() {
        let _env_guard = crate::ENV_TEST_LOCK.lock().await;
        std::env::set_var("MODAL_URL", "https://financiar186--{endpoint}.modal.run");
        std::env::set_var("MODAL_GARCH_URL", "http://localhost:9999/garch");

        assert_eq!(GARCH.url(), "http://localhost:9999/garch");

        std::env::remove_var("MODAL_URL");
        std::env::remove_var("MODAL_GARCH_URL");
    }

    #[tokio::test]
    async fn falls_back_to_the_default_workspace_without_modal_url() {
        let _env_guard = crate::ENV_TEST_LOCK.lock().await;
        std::env::remove_var("MODAL_URL");
        std::env::remove_var("MODAL_LSTM_PREPARE_URL");

        assert_eq!(
            LSTM_PREPARE.url(),
            "https://matimorales01--lstm-trend-model-prepare.modal.run"
        );
    }
}
