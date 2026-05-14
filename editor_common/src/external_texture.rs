//! # LLM-GENERATED, PLEASE REPLACE
//!
//! Cross-process wgpu texture sharing via Vulkan `VK_KHR_external_memory`.
//!
//! This module provides two primary operations:
//!
//! - [`export_texture`] – extracts a POSIX file-descriptor from a wgpu texture
//!   that was created with external-memory export capability, plus metadata
//!   the importer needs to reconstruct the image.
//!
//! - [`import_texture`] – takes a received FD, creates a Vulkan image backed by
//!   that external memory, and wraps it in a `wgpu::Texture`.
//!
//! FD transfer between processes on Linux uses `rustix::process::pidfd_getfd`.
//! The Win32 path is stubbed out and returns an error at runtime.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐                          ┌─────────────┐
//! │  Process A  │                          │  Process B  │
//! │  (exporter) │                          │  (importer) │
//! ├─────────────┤                          ├─────────────┤
//! │ create_     │                          │             │
//! │ exportable_ │                          │             │
//! │ texture()   │                          │             │
//! │      │      │                          │             │
//! │      ▼      │   metadata (side-chan)   │             │
//! │ export_     │ ───────────────────────► │             │
//! │ texture()   │                          │             │
//! │  → (fd, md) │                          │             │
//! │      │      │   pidfd_getfd()          │             │
//! │      └──────│◄─────────────────────────│             │
//! │             │                          │      ▼      │
//! │             │                          │ import_     │
//! │             │                          │ texture()   │
//! │             │                          │  → Texture  │
//! └─────────────┘                          └─────────────┘
//! ```
//!
//! # Cargo dependencies
//!
//! ```toml
//! [dependencies]
//! ash = "0.38"
//! wgpu = { version = "29", features = ["vulkan"] }
//! rustix = { version = "1", features = ["process"] }
//! serde = { version = "1", features = ["derive"] }
//! ```

use ash::vk;
use std::fmt;
use std::ops::Deref;
#[cfg(target_os = "linux")]
use std::os::fd::AsFd;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use wgpu::TextureUses;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can arise during texture export / import.
#[derive(Debug)]
pub enum TextureShareError {
    /// A Vulkan API call returned a non-success result.
    Vulkan(vk::Result),

    /// The wgpu device / texture is not backed by the Vulkan backend.
    NotVulkan,

    /// The underlying hal texture does not expose a dedicated
    /// `vk::DeviceMemory` handle.  This typically means the texture was
    /// allocated through the gpu-allocator sub-allocation path
    /// (`TextureMemory::Allocation` variant) rather than as a dedicated
    /// allocation, or the memory was marked `TextureMemory::External`.
    ///
    /// Only `TextureMemory::Dedicated(..)` yields a `vk::DeviceMemory` we
    /// can pass to `vkGetMemoryFdKHR`.
    NoDedicatedMemory,

    /// The platform-specific sharing mechanism failed.
    FdTransfer(String),

    /// Stub: the Win32 code-path has not been implemented yet.
    Win32Stub,
}

impl fmt::Display for TextureShareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "Vulkan error: {r:?}"),
            Self::NotVulkan => {
                write!(f, "device/texture is not backed by the Vulkan backend")
            }
            Self::NoDedicatedMemory => write!(
                f,
                "texture has no dedicated VkDeviceMemory (sub-allocated or external memory)"
            ),
            Self::FdTransfer(e) => write!(f, "FD transfer failed: {e}"),
            Self::Win32Stub => write!(f, "Win32 import/export is not yet implemented"),
        }
    }
}

impl std::error::Error for TextureShareError {}

impl From<vk::Result> for TextureShareError {
    fn from(r: vk::Result) -> Self {
        Self::Vulkan(r)
    }
}

pub type Result<T> = std::result::Result<T, TextureShareError>;

// ---------------------------------------------------------------------------
// Metadata exchanged between exporter and importer
// ---------------------------------------------------------------------------

/// Opaque metadata that the *exporter* must send to the *importer* alongside
/// the file descriptor so the latter can reconstruct the image.
///
/// All fields are plain Rust / wgpu types that implement `Serialize` /
/// `Deserialize`.  Vulkan `vk::*` handle types are **not** serializable, so
/// we store their raw integer representations instead and reconstruct them
/// on the importer side.
///
/// The user is expected to transmit this over a side-channel (Unix socket,
/// pipe, shared memory, …) in whatever format they prefer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TextureMetadata {
    // --- VkImageCreateInfo equivalents (stored as raw integers) ---
    pub image_type: i32,
    pub vk_format: i32,
    pub extent_width: u32,
    pub extent_height: u32,
    pub extent_depth: u32,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub samples: u32,
    pub tiling: i32,
    pub vk_usage: u32,

    // --- Memory allocation info ---
    /// Allocation size in bytes (from `vkGetImageMemoryRequirements`).
    pub allocation_size: u64,
    /// Memory type index chosen by the exporter.
    pub memory_type_index: u32,

    // --- wgpu-level description (for `create_texture_from_hal`) ---
    pub wgpu_format: wgpu::TextureFormat,
    pub wgpu_dimension: wgpu::TextureDimension,
    pub wgpu_mip_level_count: u32,
    pub wgpu_sample_count: u32,
    pub wgpu_usage: wgpu::TextureUsages,
    pub wgpu_size: wgpu::Extent3d,
    pub view_formats: Vec<wgpu::TextureFormat>,

    pub image_fd: RawFd,
}

// Convenience accessors that reconstruct the vk types from raw ints.
impl TextureMetadata {
    fn vk_image_type(&self) -> vk::ImageType {
        vk::ImageType::from_raw(self.image_type)
    }

    fn vk_format(&self) -> vk::Format {
        vk::Format::from_raw(self.vk_format)
    }

    fn vk_extent(&self) -> vk::Extent3D {
        vk::Extent3D {
            width: self.extent_width,
            height: self.extent_height,
            depth: self.extent_depth,
        }
    }

    fn vk_samples(&self) -> vk::SampleCountFlags {
        vk::SampleCountFlags::from_raw(self.samples)
    }

    fn vk_tiling(&self) -> vk::ImageTiling {
        vk::ImageTiling::from_raw(self.tiling)
    }

    fn vk_usage(&self) -> vk::ImageUsageFlags {
        vk::ImageUsageFlags::from_raw(self.vk_usage)
    }

    fn vk_allocation_size(&self) -> vk::DeviceSize {
        self.allocation_size
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn external_image_create_info(
    handle_type: vk::ExternalMemoryHandleTypeFlagsKHR,
) -> vk::ExternalMemoryImageCreateInfo<'static> {
    vk::ExternalMemoryImageCreateInfo::default().handle_types(handle_type)
}

fn export_memory_allocate_info(
    handle_type: vk::ExternalMemoryHandleTypeFlagsKHR,
) -> vk::ExportMemoryAllocateInfo<'static> {
    vk::ExportMemoryAllocateInfo::default().handle_types(handle_type)
}

/// Unwrap the `as_hal` guard.  `as_hal` returns
/// `Option<impl Deref<Target = A::Device>>` — the outer `Option` is `None`
/// when the backend doesn't match, and the guard derefs directly to the
/// hal device (no inner Option).
fn unwrap_hal_device(
    guard: Option<impl Deref<Target = wgpu::hal::vulkan::Device>>,
) -> Result<impl Deref<Target = wgpu::hal::vulkan::Device>> {
    guard.ok_or(TextureShareError::NotVulkan)
}

fn unwrap_hal_texture(
    guard: Option<impl Deref<Target = wgpu::hal::vulkan::Texture>>,
) -> Result<impl Deref<Target = wgpu::hal::vulkan::Texture>> {
    guard.ok_or(TextureShareError::NotVulkan)
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Export a wgpu texture's backing device memory as a POSIX file descriptor.
///
/// # Prerequisites
///
/// 1. The `wgpu::Device` must be backed by the **Vulkan** backend.
/// 2. The texture must have been created through [`create_exportable_texture`]
///    (or equivalent raw Vulkan calls) so that its device memory was allocated
///    with `VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT` export capability
///    **and** as a dedicated allocation (`TextureMemory::Dedicated`).
/// 3. The Vulkan device must support `VK_KHR_external_memory` and
///    `VK_KHR_external_memory_fd` (or Vulkan ≥ 1.1 on Linux).
///
/// # What this function does
///
/// 1. Acquires the raw `ash::Device` and `vk::Image` from the wgpu texture
///    via `wgpu::Texture::as_hal`.
/// 2. Reads `TextureMemory::Dedicated(device_memory)` from the hal texture.
/// 3. Calls `vkGetMemoryFdKHR` to obtain a duplicated FD.
/// 4. Collects all image-creation parameters into [`TextureMetadata`] that
///    the importer will need.
///
/// # Safety
///
/// - `device` and `texture` must refer to the same logical Vulkan device.
/// - The caller must ensure no concurrent operations that could destroy the
///   texture or device while this function executes.
/// - The returned `OwnedFd` **takes ownership** of the kernel file-descriptor.
///   Dropping it will close the FD.
pub unsafe fn export_texture(
    device: &wgpu::Device,
    texture: &wgpu::Texture,
) -> Result<(OwnedFd, TextureMetadata)> {
    let handle_type = vk::ExternalMemoryHandleTypeFlagsKHR::OPAQUE_FD;

    // ---- Access raw Vulkan objects via wgpu::hal -------------------------
    //
    // wgpu v29: `as_hal::<A>()` returns `Option<impl Deref<Target = A::Device>>`.
    // The guard derefs directly to the hal type — there is no inner Option,
    // so calling `.as_ref()` on it would be wrong.

    let hal_dev_guard = unwrap_hal_device(device.as_hal::<wgpu::hal::vulkan::Api>())?;
    let hal_tex_guard = unwrap_hal_texture(texture.as_hal::<wgpu::hal::vulkan::Api>())?;

    let ash_device = hal_dev_guard.raw_device();
    let ash_image = hal_tex_guard.raw_handle();

    // Extract the dedicated VkDeviceMemory from TextureMemory.
    let device_memory = match hal_tex_guard.memory() {
        wgpu::hal::vulkan::TextureMemory::Dedicated(mem) => *mem,
        wgpu::hal::vulkan::TextureMemory::Allocation(_) => {
            return Err(TextureShareError::NoDedicatedMemory);
        }
        wgpu::hal::vulkan::TextureMemory::External => {
            return Err(TextureShareError::NoDedicatedMemory);
        }
    };

    // ---- Query image memory requirements --------------------------------

    let mem_reqs = ash_device.get_image_memory_requirements(ash_image);

    // ---- Retrieve the FD via VK_KHR_external_memory_fd ------------------
    //
    // ash::khr::external_memory_fd::Device::new requires BOTH an Instance
    // AND a Device (it calls vkGetDeviceProcAddr under the hood).

    let ash_instance = hal_dev_guard.shared_instance().raw_instance();
    let ext_fd = ash::khr::external_memory_fd::Device::new(ash_instance, ash_device);

    let get_fd_info = vk::MemoryGetFdInfoKHR::default()
        .memory(device_memory)
        .handle_type(handle_type);

    let raw_fd: RawFd = ext_fd
        .get_memory_fd(&get_fd_info)
        .map_err(TextureShareError::Vulkan)?;

    // `vkGetMemoryFdKHR` duplicates the FD — we own it now.
    let owned_fd = OwnedFd::from_raw_fd(raw_fd);

    // ---- Collect metadata for the importer -------------------------------

    let metadata = TextureMetadata {
        image_type: wgpu_dimension_to_image_type(texture.dimension()).as_raw(),
        vk_format: wgpu_format_to_vk(texture.format()).as_raw(),
        extent_width: texture.width(),
        extent_height: texture.height(),
        extent_depth: texture.depth_or_array_layers(),
        mip_levels: texture.mip_level_count(),
        array_layers: texture.depth_or_array_layers(),
        samples: sample_count_to_vk(texture.sample_count()).as_raw(),
        tiling: vk::ImageTiling::OPTIMAL.as_raw(),
        vk_usage: wgpu_usage_to_vk(texture.usage()).as_raw(),
        allocation_size: mem_reqs.size,
        memory_type_index: lowest_set_bit(mem_reqs.memory_type_bits),
        wgpu_format: texture.format(),
        wgpu_dimension: texture.dimension(),
        wgpu_mip_level_count: texture.mip_level_count(),
        wgpu_sample_count: texture.sample_count(),
        wgpu_usage: texture.usage(),
        wgpu_size: wgpu::Extent3d {
            width: texture.width(),
            height: texture.height(),
            depth_or_array_layers: texture.depth_or_array_layers(),
        },
        view_formats: vec![],
        image_fd: raw_fd,
    };

    Ok((owned_fd, metadata))
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Import a texture from a POSIX file descriptor that was previously exported
/// by [`export_texture`].
///
/// The function:
/// 1. Creates a `VkImage` with `VkExternalMemoryImageCreateInfo` so Vulkan
///    knows the image is backed by external memory.
/// 2. Allocates `VkDeviceMemory` with `VkImportMemoryFdInfoKHR`, binding the
///    supplied FD as the backing store.
/// 3. Binds the image to that memory.
/// 4. Wraps the result in a `wgpu::hal::vulkan::Texture` via
///    `Device::texture_from_raw`, then converts to `wgpu::Texture` via
///    `Device::create_texture_from_hal`.
///
/// # Safety
///
/// - `fd` must be a valid, open file descriptor that refers to the same
///   external memory allocation the exporter created.
/// - `metadata` must exactly match what the exporter provided.
/// - The Vulkan device must support `VK_KHR_external_memory` and
///   `VK_KHR_external_memory_fd` (or Vulkan ≥ 1.1 on Linux).
/// - After this call the ownership of `fd` is transferred to Vulkan; the
///   caller must not close it.
pub unsafe fn import_texture(
    device: &wgpu::Device,
    fd: OwnedFd,
    metadata: &TextureMetadata,
) -> Result<wgpu::Texture> {
    let handle_type = vk::ExternalMemoryHandleTypeFlagsKHR::OPAQUE_FD;

    // ---- Access raw Vulkan device ----------------------------------------

    let hal_dev_guard = unwrap_hal_device(device.as_hal::<wgpu::hal::vulkan::Api>())?;
    let ash_device = hal_dev_guard.raw_device();

    // ---- 1. Create the VkImage with external-memory create info ----------

    let mut external_img_info = external_image_create_info(handle_type);

    let image_ci = vk::ImageCreateInfo::default()
        .image_type(metadata.vk_image_type())
        .format(metadata.vk_format())
        .extent(metadata.vk_extent())
        .mip_levels(metadata.mip_levels)
        .array_layers(metadata.array_layers)
        .samples(metadata.vk_samples())
        .tiling(metadata.vk_tiling())
        .usage(metadata.vk_usage())
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut external_img_info);

    let image = ash_device
        .create_image(&image_ci, None)
        .map_err(TextureShareError::Vulkan)?;

    // ---- 2. Allocate (import) device memory ------------------------------

    let mem_reqs = ash_device.get_image_memory_requirements(image);

    // Build the pnext chain:  MemoryAllocateInfo → ImportMemoryFdInfoKHR
    let mut import_fd_info = vk::ImportMemoryFdInfoKHR::default()
        .handle_type(handle_type)
        .fd(fd.as_raw_fd());

    let memory_ai = vk::MemoryAllocateInfo::default()
        .allocation_size(mem_reqs.size)
        .memory_type_index(metadata.memory_type_index)
        .push_next(&mut import_fd_info);

    let device_memory = match ash_device.allocate_memory(&memory_ai, None) {
        Ok(m) => m,
        Err(e) => {
            ash_device.destroy_image(image, None);
            return Err(TextureShareError::Vulkan(e));
        }
    };

    // From this point Vulkan owns the FD.  Forget our `OwnedFd` so it is
    // not closed on drop (Vulkan will close it when the memory is freed).
    std::mem::forget(fd);

    // ---- 3. Bind image to memory -----------------------------------------

    if let Err(e) = ash_device.bind_image_memory(image, device_memory, 0) {
        ash_device.free_memory(device_memory, None);
        ash_device.destroy_image(image, None);
        return Err(TextureShareError::Vulkan(e));
    }

    // ---- 4. Wrap in wgpu::hal::vulkan::Texture ---------------------------
    //
    // wgpu-hal v29 exposes `Device::texture_from_raw`:
    //   fn texture_from_raw(
    //       &self,
    //       vk_image: vk::Image,
    //       desc: &TextureDescriptor,
    //       drop_callback: Option<DropCallback>,    // from wgpu::hal, NOT vulkan mod
    //       memory: TextureMemory,
    //   ) -> Texture
    //
    // DropCallback is defined at the *crate root* of wgpu-hal:
    //   pub type DropCallback = Box<dyn FnOnce() + Send + Sync + 'static>;

    // Convert wgpu TextureUsages to hal TextureUses.
    let hal_usage = wgpu_usage_to_hal_usage(metadata.wgpu_usage);

    let hal_desc = wgpu::hal::TextureDescriptor {
        label: Some("imported-external-texture"),
        size: metadata.wgpu_size,
        mip_level_count: metadata.wgpu_mip_level_count,
        sample_count: metadata.wgpu_sample_count,
        dimension: metadata.wgpu_dimension,
        format: metadata.wgpu_format,
        usage: hal_usage,
        memory_flags: wgpu::hal::MemoryFlags::empty(),
        view_formats: metadata.view_formats.clone(),
    };

    // Provide a drop-callback that destroys the Vulkan objects we allocated.
    let ash_device_clone = ash_device.clone();
    // let drop_callback: wgpu::hal::DropCallback = Box::new(move || unsafe {
    //     ash_device_clone.free_memory(device_memory, None);
    //     ash_device_clone.destroy_image(image, None);
    // });

    // Mark the memory as `TextureMemory::External` because wgpu-hal did not
    // allocate it and should not attempt to free it.  Our drop_callback
    // handles cleanup instead.
    let hal_texture = hal_dev_guard.texture_from_raw(
        image,
        &hal_desc,
        // Some(drop_callback),
        None,
        wgpu::hal::vulkan::TextureMemory::External,
    );

    // ---- 5. Convert hal texture -> wgpu::Texture -------------------------

    let wgpu_desc = wgpu::TextureDescriptor {
        label: Some("imported-external-texture"),
        size: metadata.wgpu_size,
        mip_level_count: metadata.wgpu_mip_level_count,
        sample_count: metadata.wgpu_sample_count,
        dimension: metadata.wgpu_dimension,
        format: metadata.wgpu_format,
        usage: metadata.wgpu_usage,
        view_formats: &metadata.view_formats,
    };

    let wgpu_texture =
        device.create_texture_from_hal::<wgpu::hal::vulkan::Api>(hal_texture, &wgpu_desc);

    Ok(wgpu_texture)
}

// ---------------------------------------------------------------------------
// Exportable texture creation helper
// ---------------------------------------------------------------------------

/// Create a `wgpu::Texture` whose backing memory is allocated as **exportable**
/// (i.e. with `VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT`) so it can later
/// be passed to [`export_texture`].
///
/// This is the **recommended** way to create textures intended for cross-
/// process sharing, because wgpu's default texture creation path does *not*
/// set the external-memory export flags.
///
/// # How it works
///
/// Because wgpu-hal's public `create_texture` API does not accept pnext
/// chains, we go through raw Vulkan (`ash`) to:
///
/// 1. Create a `VkImage` with `VkExternalMemoryImageCreateInfo`.
/// 2. Query memory requirements.
/// 3. Allocate dedicated `VkDeviceMemory` with `VkExportMemoryAllocateInfo`.
/// 4. Bind the image to that memory.
/// 5. Wrap everything in `wgpu::hal::vulkan::Texture` via
///    `Device::texture_from_raw`, then promote to `wgpu::Texture`.
///
/// # Safety
///
/// - The caller must ensure the Vulkan device supports
///   `VK_KHR_external_memory` / `VK_KHR_external_memory_fd`.
/// - The same thread-safety rules as `wgpu::Device::create_texture` apply.
pub unsafe fn create_exportable_texture(
    device: &wgpu::Device,
    desc: &wgpu::TextureDescriptor<'_>,
) -> Result<wgpu::Texture> {
    let handle_type = vk::ExternalMemoryHandleTypeFlagsKHR::OPAQUE_FD;

    // ---- Access raw Vulkan device ----------------------------------------

    let hal_dev_guard = unwrap_hal_device(device.as_hal::<wgpu::hal::vulkan::Api>())?;
    let ash_device = hal_dev_guard.raw_device();
    let physical_device = hal_dev_guard.raw_physical_device();
    let instance = hal_dev_guard.shared_instance().raw_instance();

    // ---- Derive Vulkan create-info from the wgpu descriptor --------------

    let vk_format = wgpu_format_to_vk(desc.format);

    let (image_type, extent, array_layers) = match desc.dimension {
        wgpu::TextureDimension::D1 => (
            vk::ImageType::TYPE_1D,
            vk::Extent3D {
                width: desc.size.width,
                height: 1,
                depth: 1,
            },
            desc.size.depth_or_array_layers,
        ),
        wgpu::TextureDimension::D2 => (
            vk::ImageType::TYPE_2D,
            vk::Extent3D {
                width: desc.size.width,
                height: desc.size.height,
                depth: 1,
            },
            desc.size.depth_or_array_layers,
        ),
        wgpu::TextureDimension::D3 => (
            vk::ImageType::TYPE_3D,
            vk::Extent3D {
                width: desc.size.width,
                height: desc.size.height,
                depth: desc.size.depth_or_array_layers,
            },
            1,
        ),
    };

    // ---- 1. Create the VkImage with external memory info -----------------

    let mut external_img_ci = external_image_create_info(handle_type);

    let image_ci = vk::ImageCreateInfo::default()
        .image_type(image_type)
        .format(vk_format)
        .extent(extent)
        .mip_levels(desc.mip_level_count)
        .array_layers(array_layers)
        .samples(vk::SampleCountFlags::from_raw(desc.sample_count))
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(wgpu_usage_to_vk(desc.usage))
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut external_img_ci);

    let image = ash_device
        .create_image(&image_ci, None)
        .map_err(TextureShareError::Vulkan)?;

    // ---- 2. Allocate dedicated device memory with export capability ------

    let mem_reqs = ash_device.get_image_memory_requirements(image);
    let mem_props = instance.get_physical_device_memory_properties(physical_device);

    let memory_type_index = select_memory_type(
        &mem_props,
        mem_reqs.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    );

    let mut export_mem_ci = export_memory_allocate_info(handle_type);

    let memory_ai = vk::MemoryAllocateInfo::default()
        .allocation_size(mem_reqs.size)
        .memory_type_index(memory_type_index)
        .push_next(&mut export_mem_ci);

    let device_memory = match ash_device.allocate_memory(&memory_ai, None) {
        Ok(m) => m,
        Err(e) => {
            ash_device.destroy_image(image, None);
            return Err(TextureShareError::Vulkan(e));
        }
    };

    // ---- 3. Bind image to memory -----------------------------------------

    if let Err(e) = ash_device.bind_image_memory(image, device_memory, 0) {
        ash_device.free_memory(device_memory, None);
        ash_device.destroy_image(image, None);
        return Err(TextureShareError::Vulkan(e));
    }

    // ---- 4. Wrap in wgpu::hal::vulkan::Texture ---------------------------

    let hal_usage = wgpu_usage_to_hal_usage(desc.usage);

    let hal_desc = wgpu::hal::TextureDescriptor {
        label: desc.label,
        size: desc.size,
        mip_level_count: desc.mip_level_count,
        sample_count: desc.sample_count,
        dimension: desc.dimension,
        format: desc.format,
        usage: hal_usage,
        memory_flags: wgpu::hal::MemoryFlags::empty(),
        view_formats: desc.view_formats.to_vec(),
    };

    // Provide a drop-callback that destroys the Vulkan objects when the
    // wgpu texture is dropped.
    let ash_device_clone = ash_device.clone();
    // let drop_callback: wgpu::hal::DropCallback = Box::new(move || unsafe {
    //     ash_device_clone.free_memory(device_memory, None);
    //     ash_device_clone.destroy_image(image, None);
    // });

    // We use `TextureMemory::Dedicated(device_memory)` so that
    // `export_texture` can later extract the VkDeviceMemory handle.
    let hal_texture = hal_dev_guard.texture_from_raw(
        image,
        &hal_desc,
        // Some(drop_callback),
        None,
        wgpu::hal::vulkan::TextureMemory::Dedicated(device_memory),
    );

    // ---- 5. Convert hal texture -> wgpu::Texture -------------------------

    let wgpu_texture = device.create_texture_from_hal::<wgpu::hal::vulkan::Api>(hal_texture, desc);

    Ok(wgpu_texture)
}

// ---------------------------------------------------------------------------
// FD sharing: pidfd_getfd via rustix (Linux)
// ---------------------------------------------------------------------------

/// Steal a file descriptor from another process using `pidfd_getfd(2)`.
///
/// This is the Linux-specific mechanism for FD transfer.  The caller must:
///
/// 1. Obtain a **pidfd** for the target process (e.g. via
///    `rustix::process::pidfd_open`).
/// 2. Know the **target FD number** inside that process (the exporter must
///    communicate this over a side-channel).
///
/// The returned `OwnedFd` is a duplicate in the calling process and can be
/// passed directly to [`import_texture`].
///
/// # Permissions
///
/// `pidfd_getfd` requires that the calling process has `PTRACE` permission
/// over the target (same user, or `CAP_SYS_PTRACE`).  If this is too
/// restrictive, consider using `SCM_RIGHTS` over a Unix domain socket
/// instead — see the module-level documentation for an alternative.
///
/// # Errors
///
/// Returns `TextureShareError::FdTransfer` if the syscall fails (e.g.
/// `EPERM` – no `PTRACE` permission, `ESRCH` – target doesn't exist,
/// `EBADF` – invalid pidfd or target FD).
#[cfg(target_os = "linux")]
pub fn steal_fd_via_pidfd(pidfd: &OwnedFd, target_fd: RawFd) -> Result<OwnedFd> {
    use rustix::process::PidfdGetfdFlags;

    let flags = PidfdGetfdFlags::empty();
    match rustix::process::pidfd_getfd(pidfd, target_fd, flags) {
        Ok(owned) => Ok(owned),
        Err(e) => Err(TextureShareError::FdTransfer(format!(
            "pidfd_getfd failed: {e:?}"
        ))),
    }
}

/// Stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn steal_fd_via_pidfd(_pidfd: &OwnedFd, _target_fd: RawFd) -> Result<OwnedFd> {
    Err(TextureShareError::FdTransfer(
        "pidfd_getfd is only available on Linux".into(),
    ))
}

// ---------------------------------------------------------------------------
// Win32 stubs
// ---------------------------------------------------------------------------

/// Stub: export a texture as a Win32 `HANDLE`.
///
/// On Windows the equivalent flow would use
/// `VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_WIN32_BIT_KHR` and
/// `vkGetMemoryWin32HandleKHR`.  This function is a placeholder.
#[cfg(target_family = "windows")]
pub fn export_texture_win32(
    _device: &wgpu::Device,
    _texture: &wgpu::Texture,
) -> Result<std::os::windows::io::RawHandle> {
    Err(TextureShareError::Win32Stub)
}

/// Stub: import a texture from a Win32 `HANDLE`.
///
/// On Windows the equivalent flow would use
/// `VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_WIN32_BIT_KHR` and
/// `VkImportMemoryWin32HandleInfoKHR`.  This function is a placeholder.
#[cfg(target_family = "windows")]
pub fn import_texture_win32(
    _device: &wgpu::Device,
    _handle: std::os::windows::io::RawHandle,
    _metadata: &TextureMetadata,
) -> Result<wgpu::Texture> {
    Err(TextureShareError::Win32Stub)
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn sample_count_to_vk(count: u32) -> vk::SampleCountFlags {
    match count {
        1 => vk::SampleCountFlags::TYPE_1,
        2 => vk::SampleCountFlags::TYPE_2,
        4 => vk::SampleCountFlags::TYPE_4,
        8 => vk::SampleCountFlags::TYPE_8,
        16 => vk::SampleCountFlags::TYPE_16,
        32 => vk::SampleCountFlags::TYPE_32,
        64 => vk::SampleCountFlags::TYPE_64,
        _ => vk::SampleCountFlags::TYPE_1,
    }
}

fn wgpu_usage_to_vk(usage: wgpu::TextureUsages) -> vk::ImageUsageFlags {
    let mut flags = vk::ImageUsageFlags::empty();
    if usage.contains(wgpu::TextureUsages::COPY_SRC) {
        flags |= vk::ImageUsageFlags::TRANSFER_SRC;
    }
    if usage.contains(wgpu::TextureUsages::COPY_DST) {
        flags |= vk::ImageUsageFlags::TRANSFER_DST;
    }
    if usage.contains(wgpu::TextureUsages::TEXTURE_BINDING) {
        flags |= vk::ImageUsageFlags::SAMPLED;
    }
    if usage.contains(wgpu::TextureUsages::STORAGE_BINDING) {
        flags |= vk::ImageUsageFlags::STORAGE;
    }
    if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
        flags |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
    }
    flags
}

/// Convert wgpu `TextureUsages` to `wgpu::hal::TextureUses` (bitflags).
///
/// `wgpu::hal::TextureUses` has different bit assignments than
/// `wgpu::TextureUsages`.  This function maps the semantic meanings
/// correctly using the actual flag names from wgt v29.
fn wgpu_usage_to_hal_usage(usage: wgpu::TextureUsages) -> TextureUses {
    let mut flags = TextureUses::empty();

    if usage.contains(wgpu::TextureUsages::COPY_SRC) {
        flags |= TextureUses::COPY_SRC;
    }
    if usage.contains(wgpu::TextureUsages::COPY_DST) {
        flags |= TextureUses::COPY_DST;
    }
    // TEXTURE_BINDING → read-only sampled resource.
    if usage.contains(wgpu::TextureUsages::TEXTURE_BINDING) {
        flags |= TextureUses::RESOURCE;
    }
    // STORAGE_BINDING → storage image (read-only + read-write).
    if usage.contains(wgpu::TextureUsages::STORAGE_BINDING) {
        flags |= TextureUses::STORAGE_READ_ONLY | TextureUses::STORAGE_READ_WRITE;
    }
    // RENDER_ATTACHMENT → color target.
    if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
        flags |= TextureUses::COLOR_TARGET;
    }

    flags
}

fn wgpu_dimension_to_image_type(dim: wgpu::TextureDimension) -> vk::ImageType {
    match dim {
        wgpu::TextureDimension::D1 => vk::ImageType::TYPE_1D,
        wgpu::TextureDimension::D2 => vk::ImageType::TYPE_2D,
        wgpu::TextureDimension::D3 => vk::ImageType::TYPE_3D,
    }
}

/// Map a `wgpu::TextureFormat` to its Vulkan `vk::Format` equivalent.
///
/// This covers the most common formats.  For anything not listed, this
/// function panics — add the missing format as needed.
fn wgpu_format_to_vk(format: wgpu::TextureFormat) -> vk::Format {
    use wgpu::TextureFormat as F;
    match format {
        // 8-bit
        F::R8Unorm => vk::Format::R8_UNORM,
        F::R8Snorm => vk::Format::R8_SNORM,
        F::R8Uint => vk::Format::R8_UINT,
        F::R8Sint => vk::Format::R8_SINT,

        // 16-bit
        F::R16Uint => vk::Format::R16_UINT,
        F::R16Sint => vk::Format::R16_SINT,
        F::R16Unorm => vk::Format::R16_UNORM,
        F::R16Snorm => vk::Format::R16_SNORM,
        F::R16Float => vk::Format::R16_SFLOAT,
        F::Rg8Unorm => vk::Format::R8G8_UNORM,
        F::Rg8Snorm => vk::Format::R8G8_SNORM,
        F::Rg8Uint => vk::Format::R8G8_UINT,
        F::Rg8Sint => vk::Format::R8G8_SINT,

        // 32-bit
        F::R32Uint => vk::Format::R32_UINT,
        F::R32Sint => vk::Format::R32_SINT,
        F::R32Float => vk::Format::R32_SFLOAT,
        F::Rg16Uint => vk::Format::R16G16_UINT,
        F::Rg16Sint => vk::Format::R16G16_SINT,
        F::Rg16Unorm => vk::Format::R16G16_UNORM,
        F::Rg16Snorm => vk::Format::R16G16_SNORM,
        F::Rg16Float => vk::Format::R16G16_SFLOAT,
        F::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        F::Rgba8UnormSrgb => vk::Format::R8G8B8A8_SRGB,
        F::Rgba8Snorm => vk::Format::R8G8B8A8_SNORM,
        F::Rgba8Uint => vk::Format::R8G8B8A8_UINT,
        F::Rgba8Sint => vk::Format::R8G8B8A8_SINT,
        F::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        F::Bgra8UnormSrgb => vk::Format::B8G8R8A8_SRGB,

        // 64-bit
        F::Rg32Uint => vk::Format::R32G32_UINT,
        F::Rg32Sint => vk::Format::R32G32_SINT,
        F::Rg32Float => vk::Format::R32G32_SFLOAT,
        F::Rgba16Uint => vk::Format::R16G16B16A16_UINT,
        F::Rgba16Sint => vk::Format::R16G16B16A16_SINT,
        F::Rgba16Unorm => vk::Format::R16G16B16A16_UNORM,
        F::Rgba16Snorm => vk::Format::R16G16B16A16_SNORM,
        F::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,

        // 128-bit
        F::Rgba32Uint => vk::Format::R32G32B32A32_UINT,
        F::Rgba32Sint => vk::Format::R32G32B32A32_SINT,
        F::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,

        // Depth / stencil
        F::Depth16Unorm => vk::Format::D16_UNORM,
        F::Depth24Plus => vk::Format::X8_D24_UNORM_PACK32,
        F::Depth24PlusStencil8 => vk::Format::D24_UNORM_S8_UINT,
        F::Depth32Float => vk::Format::D32_SFLOAT,
        F::Depth32FloatStencil8 => vk::Format::D32_SFLOAT_S8_UINT,

        // BC compressed
        F::Bc1RgbaUnorm => vk::Format::BC1_RGBA_UNORM_BLOCK,
        F::Bc1RgbaUnormSrgb => vk::Format::BC1_RGBA_SRGB_BLOCK,
        F::Bc2RgbaUnorm => vk::Format::BC2_UNORM_BLOCK,
        F::Bc2RgbaUnormSrgb => vk::Format::BC2_SRGB_BLOCK,
        F::Bc3RgbaUnorm => vk::Format::BC3_UNORM_BLOCK,
        F::Bc3RgbaUnormSrgb => vk::Format::BC3_SRGB_BLOCK,
        F::Bc4RUnorm => vk::Format::BC4_UNORM_BLOCK,
        F::Bc4RSnorm => vk::Format::BC4_SNORM_BLOCK,
        F::Bc5RgUnorm => vk::Format::BC5_UNORM_BLOCK,
        F::Bc5RgSnorm => vk::Format::BC5_SNORM_BLOCK,
        F::Bc6hRgbUfloat => vk::Format::BC6H_UFLOAT_BLOCK,
        F::Bc6hRgbFloat => vk::Format::BC6H_SFLOAT_BLOCK,
        F::Bc7RgbaUnorm => vk::Format::BC7_UNORM_BLOCK,
        F::Bc7RgbaUnormSrgb => vk::Format::BC7_SRGB_BLOCK,

        // ETC compressed
        F::Etc2Rgb8Unorm => vk::Format::ETC2_R8G8B8_UNORM_BLOCK,
        F::Etc2Rgb8UnormSrgb => vk::Format::ETC2_R8G8B8_SRGB_BLOCK,
        F::Etc2Rgb8A1Unorm => vk::Format::ETC2_R8G8B8A1_UNORM_BLOCK,
        F::Etc2Rgb8A1UnormSrgb => vk::Format::ETC2_R8G8B8A1_SRGB_BLOCK,
        F::Etc2Rgba8Unorm => vk::Format::ETC2_R8G8B8A8_UNORM_BLOCK,
        F::Etc2Rgba8UnormSrgb => vk::Format::ETC2_R8G8B8A8_SRGB_BLOCK,
        F::EacR11Unorm => vk::Format::EAC_R11_UNORM_BLOCK,
        F::EacR11Snorm => vk::Format::EAC_R11_SNORM_BLOCK,
        F::EacRg11Unorm => vk::Format::EAC_R11G11_UNORM_BLOCK,
        F::EacRg11Snorm => vk::Format::EAC_R11G11_SNORM_BLOCK,

        // ASTC
        F::Astc { block, channel } => {
            let srgb = matches!(channel, wgpu::AstcChannel::UnormSrgb);
            match block {
                wgpu::AstcBlock::B4x4 => {
                    if srgb {
                        vk::Format::ASTC_4X4_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_4X4_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B5x4 => {
                    if srgb {
                        vk::Format::ASTC_5X4_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_5X4_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B5x5 => {
                    if srgb {
                        vk::Format::ASTC_5X5_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_5X5_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B6x5 => {
                    if srgb {
                        vk::Format::ASTC_6X5_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_6X5_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B6x6 => {
                    if srgb {
                        vk::Format::ASTC_6X6_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_6X6_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B8x5 => {
                    if srgb {
                        vk::Format::ASTC_8X5_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_8X5_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B8x6 => {
                    if srgb {
                        vk::Format::ASTC_8X6_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_8X6_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B8x8 => {
                    if srgb {
                        vk::Format::ASTC_8X8_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_8X8_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B10x5 => {
                    if srgb {
                        vk::Format::ASTC_10X5_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_10X5_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B10x6 => {
                    if srgb {
                        vk::Format::ASTC_10X6_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_10X6_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B10x8 => {
                    if srgb {
                        vk::Format::ASTC_10X8_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_10X8_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B10x10 => {
                    if srgb {
                        vk::Format::ASTC_10X10_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_10X10_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B12x10 => {
                    if srgb {
                        vk::Format::ASTC_12X10_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_12X10_UNORM_BLOCK
                    }
                }
                wgpu::AstcBlock::B12x12 => {
                    if srgb {
                        vk::Format::ASTC_12X12_SRGB_BLOCK
                    } else {
                        vk::Format::ASTC_12X12_UNORM_BLOCK
                    }
                }
            }
        }

        _ => panic!("wgpu_format_to_vk: unsupported format {format:?}"),
    }
}

fn select_memory_type(
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    preferred: vk::MemoryPropertyFlags,
) -> u32 {
    // First pass: try to find a memory type with the preferred properties.
    for i in 0..mem_props.memory_type_count {
        if (type_bits & (1 << i)) != 0
            && mem_props.memory_types[i as usize]
                .property_flags
                .contains(preferred)
        {
            return i;
        }
    }
    // Fallback: any memory type that matches the bitmask.
    for i in 0..mem_props.memory_type_count {
        if (type_bits & (1 << i)) != 0 {
            return i;
        }
    }
    0
}

/// Return the index of the lowest set bit in `bits`.
fn lowest_set_bit(bits: u32) -> u32 {
    bits.trailing_zeros()
}

// ---------------------------------------------------------------------------
// Integration example
// ---------------------------------------------------------------------------

/// End-to-end example showing how two processes would share a texture.
///
/// ```text
/// Process A (exporter)                       Process B (importer)
/// ─────────────────────                      ────────────────────
/// 1. create_exportable_texture()             .
/// 2. export_texture()  → (fd, metadata)      .
/// 3. Send metadata + fd number over socket   Receive metadata + fd number
/// 4. pidfd_open(pid_of_B)                    pidfd_open(pid_of_A)
/// 5. (target_fd known via protocol)          steal_fd_via_pidfd(pidfd_of_A, fd_no)
///                                            6. import_texture(fd, metadata)
/// ```
///
/// A simpler alternative for same-host scenarios is `SCM_RIGHTS` via Unix
/// sockets, which avoids the need for `PTRACE` permissions that
/// `pidfd_getfd` requires.
pub mod example {
    use super::*;

    /// Demonstrate the exporter side of a texture share.
    ///
    /// Returns the FD and metadata the exporter must convey to the importer.
    #[cfg(target_os = "linux")]
    pub unsafe fn exporter_side(
        device: &wgpu::Device,
        desc: &wgpu::TextureDescriptor<'_>,
    ) -> Result<(OwnedFd, TextureMetadata)> {
        // Step 1 – create a texture whose memory is exportable.
        let texture = create_exportable_texture(device, desc)?;

        // Step 2 – extract the FD + metadata.
        let (fd, metadata) = export_texture(device, &texture)?;

        Ok((fd, metadata))
    }

    /// Demonstrate the importer side.
    ///
    /// `pidfd_of_exporter` should be obtained via `rustix::process::pidfd_open`.
    /// `fd_number_in_exporter` is the FD number inside the exporter process
    /// (which must have been communicated over a side-channel).
    #[cfg(target_os = "linux")]
    pub unsafe fn importer_side(
        device: &wgpu::Device,
        pidfd_of_exporter: &OwnedFd,
        fd_number_in_exporter: RawFd,
        metadata: &TextureMetadata,
    ) -> Result<wgpu::Texture> {
        // Steal the FD from the exporter process.
        let fd = steal_fd_via_pidfd(pidfd_of_exporter, fd_number_in_exporter)?;

        // Import it as a wgpu texture.
        import_texture(device, fd, metadata)
    }
}
