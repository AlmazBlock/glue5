//! Why the GPU was lost, where the Vulkan driver can say (VK_EXT_device_fault,
//! on Mali and Adreno among others): the device is opened with the extension
//! and a lost device logs the driver's fault report (description, faulting
//! addresses and accesses, vendor codes). wgpu only reports "Unknown".

use anyhow::{Context, Result};
use ash::vk;
use wgpu::hal::api::Vulkan;

/// What reads a lost device's fault report.
pub struct Faults(ash::ext::device_fault::Device);

/// The device for `desc`, with fault reports where the driver has them.
pub fn request_device(
    adapter: &wgpu::Adapter,
    desc: &wgpu::DeviceDescriptor,
) -> Result<(wgpu::Device, wgpu::Queue, Option<Faults>)> {
    // SAFETY: the hal adapter is used only while `adapter` lives.
    let supported = unsafe { adapter.as_hal::<Vulkan>() }
        .is_some_and(|a| a.physical_device_capabilities().supports_extension(ash::ext::device_fault::NAME));
    if !supported {
        let (device, queue) = pollster::block_on(adapter.request_device(desc)).context("creating the GPU device")?;
        return Ok((device, queue, None));
    }
    let open = {
        let hal = unsafe { adapter.as_hal::<Vulkan>() }.context("no Vulkan adapter")?;
        let mut feature = vk::PhysicalDeviceFaultFeaturesEXT::default().device_fault(true);
        let feature = &mut feature;
        let callback: Box<wgpu::hal::vulkan::CreateDeviceCallback<'_>> = Box::new(move |args| {
            args.extensions.push(ash::ext::device_fault::NAME);
            let info = std::mem::take(args.create_info);
            *args.create_info = info.push_next(feature);
        });
        // SAFETY: only adds an extension the device supports and its feature.
        unsafe { hal.open_with_callback(desc.required_features, &desc.required_limits, &desc.memory_hints, Some(callback)) }
            .context("creating the GPU device")?
    };
    // SAFETY: opened from this adapter, with the features asked.
    let (device, queue) = unsafe { adapter.create_device_from_hal(open, desc) }.context("creating the GPU device")?;
    let faults = {
        let hal_adapter = unsafe { adapter.as_hal::<Vulkan>() }.context("no Vulkan adapter")?;
        let hal_device = unsafe { device.as_hal::<Vulkan>() }.context("no Vulkan device")?;
        Faults(ash::ext::device_fault::Device::new(hal_adapter.shared_instance().raw_instance(), hal_device.raw_device()))
    };
    Ok((device, queue, Some(faults)))
}

impl Faults {
    /// Logs the driver's report on the lost device.
    pub fn report(&self) {
        let get = self.0.fp().get_device_fault_info_ext;
        let mut counts = vk::DeviceFaultCountsEXT::default();
        // SAFETY: counts first, then arrays of those sizes (no vendor binary).
        if unsafe { get(self.0.device(), &mut counts, std::ptr::null_mut()) } != vk::Result::SUCCESS {
            log::error!("gpu: no fault report");
            return;
        }
        counts.vendor_binary_size = 0;
        let mut addresses = vec![vk::DeviceFaultAddressInfoEXT::default(); counts.address_info_count as usize];
        let mut vendor = vec![vk::DeviceFaultVendorInfoEXT::default(); counts.vendor_info_count as usize];
        let mut info = vk::DeviceFaultInfoEXT {
            p_address_infos: addresses.as_mut_ptr(),
            p_vendor_infos: vendor.as_mut_ptr(),
            ..Default::default()
        };
        let result = unsafe { get(self.0.device(), &mut counts, &mut info) };
        if result != vk::Result::SUCCESS && result != vk::Result::INCOMPLETE {
            log::error!("gpu: no fault report ({result:?})");
            return;
        }
        log::error!("gpu: fault report: {}", text(info.description_as_c_str().ok()));
        for a in &addresses[..counts.address_info_count as usize] {
            log::error!(
                "gpu: fault at {:#x} (± {:#x}): {:?}",
                a.reported_address,
                a.address_precision,
                a.address_type
            );
        }
        for v in &vendor[..counts.vendor_info_count as usize] {
            log::error!(
                "gpu: fault (vendor) {}: code {:#x}, data {:#x}",
                text(v.description_as_c_str().ok()),
                v.vendor_fault_code,
                v.vendor_fault_data
            );
        }
    }
}

fn text(s: Option<&std::ffi::CStr>) -> String {
    s.map_or_else(|| "?".into(), |s| s.to_string_lossy().into_owned())
}
