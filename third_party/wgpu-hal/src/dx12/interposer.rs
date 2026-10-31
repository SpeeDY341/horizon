//! Optional, instance-scoped DX12 creation interception. Normal instances never call it.
use super::*;
use alloc::string::ToString;

/// The native interface passed to a creation interposer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationInterface {
    Factory,
    Device,
}

type Upgrade =
    dyn Fn(*mut ffi::c_void, CreationInterface) -> Result<*mut ffi::c_void, String> + Send + Sync;

/// Owns the callback and its captured SDK lifetime, after all COM fields using it.
#[derive(Clone)]
pub struct CreationHook {
    upgrade: Arc<Upgrade>,
    // SDK shutdown and final native device Release also need d3d12.dll loaded.
    // Callback (and its SDK guard) drops before this library reference.
    library: Option<Arc<D3D12Lib>>,
}

impl CreationHook {
    /// # Safety
    /// Input is borrowed. Return the input unchanged, or one owned IUnknown
    /// reference to a proxy supporting the requested interface. Errors transfer
    /// no reference. Captures must keep its implementation loaded until dropped.
    pub unsafe fn new(
        upgrade: impl Fn(*mut ffi::c_void, CreationInterface) -> Result<*mut ffi::c_void, String>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            upgrade: Arc::new(upgrade),
            library: None,
        }
    }

    pub(super) unsafe fn upgrade<T: Interface>(
        &self,
        native: &T,
        interface: CreationInterface,
    ) -> Result<T, String> {
        let pointer = (self.upgrade)(native.as_raw(), interface)?;
        if pointer.is_null() {
            return Err("creation interposer returned null".into());
        }
        if pointer == native.as_raw() {
            return Ok(native.clone());
        }
        // QueryInterface establishes the exact requested version and owns a new
        // reference. Release the callback's owned reference on success OR failure.
        let proxy = unsafe { windows_core::IUnknown::from_raw(pointer) };
        proxy.cast::<T>().map_err(|error| error.to_string())
    }
}

impl Instance {
    /// Retain D3D12's implementation independently of HAL object ownership.
    /// Native integrations holding additional COM references must keep this
    /// guard until their shutdown and final native Release have completed.
    pub fn native_library_guard(&self) -> Arc<dyn Send + Sync> {
        self.library.clone()
    }

    /// Initialize before any surfaces/adapters exist, so every queue and chain
    /// is created through the interposer, including recreated surfaces.
    ///
    /// # Safety
    /// The hook must obey [`CreationHook::new`]'s interface/lifetime contract.
    pub unsafe fn init_with_creation_hook(
        desc: &crate::InstanceDescriptor<'_>,
        mut hook: CreationHook,
    ) -> Result<Self, crate::InstanceError> {
        let mut instance = unsafe { <Self as crate::Instance>::init(desc) }?;
        hook.library = Some(instance.library.clone());
        let factory = unsafe { hook.upgrade(instance.factory.deref(), CreationInterface::Factory) }
            .map_err(crate::InstanceError::new)?;
        instance.factory = DxgiFactory::Factory4(factory);
        instance.creation_hook = Some(hook);
        Ok(instance)
    }
}
