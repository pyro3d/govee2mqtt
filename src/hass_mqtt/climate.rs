use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::EntityInstance;
use crate::hass_mqtt::number::NumberConfig;
use crate::platform_api::{DeviceCapability, DeviceParameters};
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, topic_safe_id, topic_safe_string, HassClient};
use crate::service::state::StateHandle;
use crate::temperature::{
    TemperatureScale, TemperatureUnits, TemperatureValue, DEVICE_CLASS_TEMPERATURE,
};
use anyhow::anyhow;
use axum::async_trait;
use mosquitto_rs::router::{Params, Payload, State};
use serde::Deserialize;

// TODO: register an actual climate entity.
// I don't have one of these devices, so it is currently guesswork!

pub struct TargetTemperatureEntity {
    number: NumberConfig,
}

pub struct TemperatureConstraints {
    pub min: TemperatureValue,
    pub max: TemperatureValue,
}

impl TemperatureConstraints {
    pub fn as_unit(&self, unit: TemperatureUnits) -> Self {
        Self {
            min: self.min.as_unit(unit),
            max: self.max.as_unit(unit),
        }
    }
}

pub fn parse_temperature_constraints(
    instance: &DeviceCapability,
) -> anyhow::Result<TemperatureConstraints> {
    let units = instance
        .struct_field_by_name("unit")
        .map(
            |field| match field.default_value.as_ref().and_then(|v| v.as_str()) {
                Some("Celsius") => TemperatureUnits::Celsius,
                Some("Farenheit") => TemperatureUnits::Farenheit,
                _ => TemperatureUnits::Farenheit,
            },
        )
        .unwrap_or(TemperatureUnits::Farenheit);

    let temperature = instance
        .struct_field_by_name("temperature")
        .ok_or_else(|| anyhow!("no temperature field in {instance:?}"))?;
    match &temperature.field_type {
        DeviceParameters::Integer { unit, range } => {
            let range_units = match unit.as_deref() {
                Some("Celsius") => TemperatureUnits::Celsius,
                Some("Farenheit") => TemperatureUnits::Farenheit,
                _ => units,
            };

            let min = TemperatureValue::new(range.min.into(), range_units);
            let max = TemperatureValue::new(range.max.into(), range_units);

            Ok(TemperatureConstraints {
                min: min.as_unit(units),
                max: max.as_unit(units),
            })
        }
        _ => {
            anyhow::bail!("Unexpected temperature value in {instance:?}");
        }
    }
}

impl TargetTemperatureEntity {
    pub async fn new(
        device: &ServiceDevice,
        state: &StateHandle,
        instance: &DeviceCapability,
    ) -> anyhow::Result<Self> {
        let units = state.get_temperature_scale().await;

        let constraints = parse_temperature_constraints(instance)?.as_unit(units.into());
        let unique_id = format!(
            "{id}-{inst}",
            id = topic_safe_id(device),
            inst = topic_safe_string(&instance.instance)
        );

        let name = "Target Temperature".to_string();
        let command_topic = format!(
            "gv2mqtt/{id}/set-temperature/{inst}/{units}",
            id = topic_safe_id(device),
            inst = topic_safe_string(&instance.instance)
        );

        Ok(Self {
            number: NumberConfig {
                base: EntityConfig {
                    availability_topic: availability_topic(),
                    name: Some(name),
                    entity_category: None,
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id: unique_id.clone(),
                    device_class: Some(DEVICE_CLASS_TEMPERATURE),
                    icon: Some("mdi:thermometer".to_string()),
                },
                state_topic: None,
                command_topic,
                min: Some(constraints.min.value().floor() as f32),
                max: Some(constraints.max.value().ceil() as f32),
                step: 1.0,
                unit_of_measurement: Some(units.unit_of_measurement()),
            },
        })
    }
}

#[async_trait]
impl EntityInstance for TargetTemperatureEntity {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.number.publish(&state, &client).await
    }

    async fn notify_state(&self, _client: &HassClient) -> anyhow::Result<()> {
        // No state to publish
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct IdInstAndUnits {
    id: String,
    instance: String,
    units: String,
}

pub async fn mqtt_set_temperature(
    Payload(value): Payload<String>,
    Params(IdInstAndUnits {
        id,
        instance,
        units,
    }): Params<IdInstAndUnits>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("Command: set-temperature for {id}: {value}");
    let device = state
        .resolve_device(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("device '{id}' not found"))?;

    let scale: TemperatureScale = units.parse()?;
    let target_value = TemperatureValue::parse_with_optional_scale(&value, Some(scale))?;

    state
        .device_set_target_temperature(&device, &instance, target_value)
        .await?;

    Ok(())
}
