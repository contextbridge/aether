use std::str::FromStr;

use super::{BedrockFoundationModel, ModelSource};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BedrockModel {
    Foundation(BedrockFoundationModel),
    Profile(String),
}

impl BedrockModel {
    pub(super) fn source(&self) -> ModelSource<'_> {
        match self {
            Self::Foundation(model) => ModelSource::Catalog(model.entry()),
            Self::Profile(id) => ModelSource::Runtime(id),
        }
    }
}

impl FromStr for BedrockModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(model) = BedrockFoundationModel::ALL.iter().copied().find(|model| model.entry().id == s) {
            Ok(Self::Foundation(model))
        } else if is_bedrock_inference_profile_arn(s) {
            Err("Bedrock inference profile ARNs must be configured as providers.bedrock.inferenceProfileArn; keep model as bedrock:<model-id>".to_string())
        } else {
            Ok(Self::Profile(s.to_string()))
        }
    }
}

fn is_bedrock_inference_profile_arn(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("arn:") else {
        return false;
    };
    let parts: Vec<&str> = rest.split(':').collect();
    matches!(
        parts.as_slice(),
        [partition, "bedrock", _, _, resource, ..]
            if partition.starts_with("aws")
                && (resource.starts_with("inference-profile/")
                    || resource.starts_with("application-inference-profile/"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foundation_model_parses() {
        let model: BedrockModel = "anthropic.claude-sonnet-4-5-20250929-v1:0".parse().unwrap();
        assert!(matches!(model, BedrockModel::Foundation(_)));
    }

    #[test]
    fn unknown_profile_id_falls_through_to_profile_variant() {
        let model: BedrockModel = "us.anthropic.claude-future-model-v99:0".parse().unwrap();
        assert!(matches!(model, BedrockModel::Profile(_)));
        assert_eq!(crate::LlmModel::Bedrock(model).context_window(), None);
    }

    #[test]
    fn inference_profile_arn_is_rejected() {
        let error =
            "arn:aws:bedrock:us-west-2:000000000000:inference-profile/us.anthropic.claude-sonnet-4-5-20250929-v1:0"
                .parse::<BedrockModel>()
                .unwrap_err();
        assert!(error.contains("providers.bedrock.inferenceProfileArn"));
    }

    #[test]
    fn application_inference_profile_arn_is_rejected() {
        let error = "arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000"
            .parse::<BedrockModel>()
            .unwrap_err();
        assert!(error.contains("providers.bedrock.inferenceProfileArn"));
    }

    #[test]
    fn gov_cloud_arn_is_rejected() {
        let error = "arn:aws-us-gov:bedrock:us-gov-west-1:000000000000:application-inference-profile/000000000000"
            .parse::<BedrockModel>()
            .unwrap_err();
        assert!(error.contains("providers.bedrock.inferenceProfileArn"));
    }

    #[test]
    fn non_bedrock_arn_falls_through_to_profile() {
        let model: BedrockModel = "arn:aws:s3:us-west-2:000000000000:bucket/foo".parse().unwrap();
        assert!(matches!(model, BedrockModel::Profile(_)));
    }
}
