use anyhow::Context;
use k8s_openapi::{api::core::v1::Secret, serde_json};
use kube::{
    api::{ApiResource, DynamicObject, GroupVersionKind, Patch},
    discovery::{ApiCapabilities, Scope},
    Api, Client, Discovery, ResourceExt,
};
use log::error;
use log::info;
use serde_yaml::Value;

fn dynamic_api(
    ar: ApiResource,
    caps: ApiCapabilities,
    client: Client,
    ns: Option<&str>,
    all: bool,
) -> Api<DynamicObject> {
    if caps.scope == Scope::Cluster || all {
        Api::all_with(client, &ar)
    } else if let Some(namespace) = ns {
        Api::namespaced_with(client, namespace, &ar)
    } else {
        Api::default_namespaced_with(client, &ar)
    }
}

pub async fn get_database_password(kubeclient: &Client, instance_name: &str) -> Result<String, ()> {
    let sec_api: Api<Secret> = kube::Api::namespaced(kubeclient.clone(), "moonscale");
    let database_sec = sec_api
        .get(format!("moonscale-instance-{}", instance_name).as_str())
        .await;

    if database_sec.is_err() {
        error!("Failed to get secret for database {}", instance_name);
        return Err(());
    }

    let database_secret_kv = database_sec.unwrap().data.unwrap();
    let root_password_kv = database_secret_kv.get("mysql-root-password");

    if root_password_kv.is_none() {
        error!("Failed to get root password for database {}", instance_name);
        return Err(());
    }

    Ok(String::from_utf8(root_password_kv.unwrap().0.clone()).unwrap())
}

pub async fn kubernetes_apply_document(
    kubeclient: &Client,
    api_discovery: &Discovery,
    patch_params: &kube::api::PatchParams,
    doc: Value,
) -> Result<(), anyhow::Error> {
    let obj: DynamicObject = serde_yaml::from_value(doc)?;
    let namespace = obj.metadata.namespace.as_deref().or(Some("moonscale"));
    let type_meta = obj.types.as_ref();

    if type_meta.is_none() {
        return Err(anyhow::anyhow!("Document has no type metadata"));
    }

    let gvk = GroupVersionKind::try_from(type_meta.unwrap()).context("Failed to get GVK")?;
    let name = obj.name_any();

    if let Some((ar, caps)) = api_discovery.resolve_gvk(&gvk) {
        let api = dynamic_api(ar, caps, kubeclient.clone(), namespace, false);
        let data: serde_json::Value =
            serde_json::to_value(&obj).context("Failed to serialize object to JSON")?;
        let api_patch_result = api.patch(&name, patch_params, &Patch::Apply(data)).await;

        // TODO: Add system labels injection (managed-by...)
        if api_patch_result.is_err() {
            error!(
                "Failed to apply document for {:?}: {:?}",
                name, api_patch_result
            );
            return Err(api_patch_result.err().unwrap().into());
        }
        info!("Applied {:?}", &obj.types.unwrap());
    } else {
        error!(
            "Cannot apply document for unknown type {:?}",
            &obj.types.unwrap()
        );
        return Err(anyhow::anyhow!("Unknown type"));
    }
    Ok(())
}
