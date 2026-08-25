// Copyright 2026 The ChromiumOS Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(feature = "gfxstream")]
use std::collections::BTreeMap as Map;

#[cfg(feature = "gfxstream")]
use magma_gpu::util::AsBorrowedDescriptor;
#[cfg(feature = "gfxstream")]
use magma_gpu::util::AsRawDescriptor;
use magma_gpu::util::Error as MagmaGpuError;
#[cfg(feature = "gfxstream")]
use magma_gpu::util::Handle as MagmaGpuHandle;
#[cfg(feature = "gfxstream")]
use magma_gpu::util::Tube;
#[cfg(feature = "gfxstream")]
use magma_gpu::util::TubeType;
#[cfg(feature = "gfxstream")]
use magma_gpu::util::MAGMA_GPU_HANDLE_TYPE_SIGNAL_EVENT_FD;
use rutabaga_gfx::Rutabaga;
#[cfg(any(feature = "gfxstream", test))]
use zerocopy::FromBytes;
#[cfg(any(feature = "gfxstream", test))]
use zerocopy::Immutable;
#[cfg(any(feature = "gfxstream", test))]
use zerocopy::IntoBytes;

use crate::kumquat_gpu::KumquatGpuResult;

#[cfg(any(feature = "gfxstream", test))]
const UDROID_AHB_TRANSPORT_MAGIC: u32 = 0x5544_4842;
#[cfg(any(feature = "gfxstream", test))]
const UDROID_AHB_TRANSPORT_VERSION: u32 = 1;
#[cfg(any(feature = "gfxstream", test))]
const UDROID_AHB_REGISTER_BUFFER: u32 = 1;
#[cfg(any(feature = "gfxstream", test))]
const UDROID_AHB_ACQUIRE_FENCE: u32 = 2;
#[cfg(any(feature = "gfxstream", test))]
const UDROID_AHB_RELEASE_FENCE: u32 = 3;

#[cfg(any(feature = "gfxstream", test))]
#[derive(Copy, Clone, Debug, FromBytes, Immutable, IntoBytes)]
#[repr(C)]
struct UdroidAhbTransportPacket {
    magic: u32,
    version: u32,
    kind: u32,
    reserved: u32,
    resource_id: u64,
    generation: u64,
}

#[cfg(feature = "gfxstream")]
pub struct PresenterClient {
    socket: Tube,
    next_generation: u64,
    registered_resources: Map<u32, u64>,
}

#[cfg(not(feature = "gfxstream"))]
pub struct PresenterClient;

impl PresenterClient {
    #[cfg(feature = "gfxstream")]
    pub fn connect(path: &str) -> KumquatGpuResult<Self> {
        Ok(Self {
            socket: Tube::new(path, TubeType::Packet)?,
            next_generation: 0,
            registered_resources: Default::default(),
        })
    }

    #[cfg(not(feature = "gfxstream"))]
    pub fn connect(_path: &str) -> KumquatGpuResult<Self> {
        Err(MagmaGpuError::Unsupported.into())
    }

    #[cfg(feature = "gfxstream")]
    pub fn register_resource(
        &mut self,
        rutabaga: &mut Rutabaga,
        resource_id: u32,
    ) -> KumquatGpuResult<()> {
        if self.registered_resources.contains_key(&resource_id) {
            return Ok(());
        }

        self.next_generation =
            self.next_generation
                .checked_add(1)
                .ok_or(MagmaGpuError::WithContext(
                    "uDroid presenter generation overflow",
                ))?;
        let packet = UdroidAhbTransportPacket {
            magic: UDROID_AHB_TRANSPORT_MAGIC,
            version: UDROID_AHB_TRANSPORT_VERSION,
            kind: UDROID_AHB_REGISTER_BUFFER,
            reserved: 0,
            resource_id: resource_id.into(),
            generation: self.next_generation,
        };
        if self.socket.send(packet.as_bytes(), &[])? != size_of_val(&packet) {
            return Err(
                MagmaGpuError::WithContext("short uDroid presenter registration write").into(),
            );
        }

        let socket_fd = self.socket.as_borrowed_descriptor().as_raw_descriptor();
        rutabaga.resource_send_hardware_buffer(resource_id, socket_fd)?;
        self.registered_resources
            .insert(resource_id, self.next_generation);
        Ok(())
    }

    #[cfg(feature = "gfxstream")]
    pub fn present_resource(
        &mut self,
        resource_id: u32,
        acquire_fence: MagmaGpuHandle,
    ) -> KumquatGpuResult<MagmaGpuHandle> {
        let generation =
            *self
                .registered_resources
                .get(&resource_id)
                .ok_or(MagmaGpuError::WithContext(
                    "uDroid presenter resource is not registered",
                ))?;
        let packet = UdroidAhbTransportPacket {
            magic: UDROID_AHB_TRANSPORT_MAGIC,
            version: UDROID_AHB_TRANSPORT_VERSION,
            kind: UDROID_AHB_ACQUIRE_FENCE,
            reserved: 0,
            resource_id: resource_id.into(),
            generation,
        };
        if self
            .socket
            .send(packet.as_bytes(), &[acquire_fence.os_handle])?
            != size_of_val(&packet)
        {
            return Err(MagmaGpuError::WithContext("short uDroid presenter acquire write").into());
        }

        let mut response_bytes = [0u8; size_of::<UdroidAhbTransportPacket>()];
        let (received, mut descriptors) = self.socket.receive(&mut response_bytes)?;
        if received != response_bytes.len() || descriptors.len() != 1 {
            return Err(
                MagmaGpuError::WithContext("malformed uDroid presenter release response").into(),
            );
        }
        let response = UdroidAhbTransportPacket::read_from_bytes(&response_bytes)
            .map_err(|_| MagmaGpuError::WithContext("invalid uDroid presenter release packet"))?;
        if response.magic != UDROID_AHB_TRANSPORT_MAGIC
            || response.version != UDROID_AHB_TRANSPORT_VERSION
            || response.kind != UDROID_AHB_RELEASE_FENCE
            || response.reserved != 0
            || response.resource_id != u64::from(resource_id)
            || response.generation != generation
        {
            return Err(MagmaGpuError::WithContext(
                "stale or mismatched uDroid presenter release packet",
            )
            .into());
        }
        Ok(MagmaGpuHandle {
            os_handle: descriptors.remove(0),
            handle_type: MAGMA_GPU_HANDLE_TYPE_SIGNAL_EVENT_FD,
        })
    }

    #[cfg(not(feature = "gfxstream"))]
    pub fn register_resource(
        &mut self,
        _rutabaga: &mut Rutabaga,
        _resource_id: u32,
    ) -> KumquatGpuResult<()> {
        Err(MagmaGpuError::Unsupported.into())
    }

    #[cfg(not(feature = "gfxstream"))]
    pub fn present_resource(
        &mut self,
        _resource_id: u32,
        _acquire_fence: magma_gpu::util::Handle,
    ) -> KumquatGpuResult<magma_gpu::util::Handle> {
        Err(MagmaGpuError::Unsupported.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_packet_matches_udroid_wire_layout() {
        assert_eq!(size_of::<UdroidAhbTransportPacket>(), 32);
        let packet = UdroidAhbTransportPacket {
            magic: UDROID_AHB_TRANSPORT_MAGIC,
            version: UDROID_AHB_TRANSPORT_VERSION,
            kind: UDROID_AHB_REGISTER_BUFFER,
            reserved: 0,
            resource_id: 0x1122_3344,
            generation: 7,
        };
        assert_eq!(&packet.as_bytes()[0..4], &0x5544_4842u32.to_ne_bytes());
        assert_eq!(&packet.as_bytes()[16..24], &0x1122_3344u64.to_ne_bytes());
    }
}
