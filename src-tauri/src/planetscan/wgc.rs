//! Windows' own window capture (Windows Graphics Capture) for the planet scan: one EVE window's
//! picture at its full size, as fast as EVE draws it, straight from the graphics card. Only the
//! inside of the window (no title bar), no mouse pointer, no yellow border where Windows allows.
//! It reads the picture Windows already composes; nothing touches the game.

use windows::core::Interface;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, POINT, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

fn why(e: windows::core::Error) -> String {
    e.message().to_string()
}

pub struct Capture {
    hwnd: HWND,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    d3d: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    _item: GraphicsCaptureItem,
    size: (i32, i32),
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// Where the window's inside sits in the captured picture (x, y, width, height), looked up
    /// again every so often (the window can move between screens or change size).
    area: Option<(u32, u32, u32, u32)>,
    area_age: u32,
}

impl Capture {
    pub fn start(handle: isize) -> Result<Capture, String> {
        // SAFETY: plain Windows calls; every object made here is owned by the Capture and released with it.
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
            let hwnd = HWND(handle as *mut core::ffi::c_void);
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(why)?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd).map_err(why)?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).map_err(why)?;
            let device: ID3D11Device = device.ok_or("no Direct3D device")?;
            let context: ID3D11DeviceContext = context.ok_or("no Direct3D context")?;
            let dxgi: IDXGIDevice = device.cast().map_err(why)?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi).map_err(why)?.cast().map_err(why)?;
            let size = item.Size().map_err(why)?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size).map_err(why)?;
            let session = pool.CreateCaptureSession(&item).map_err(why)?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture().map_err(why)?;
            Ok(Capture { hwnd, device, context, d3d, pool, session, _item: item, size: (size.Width, size.Height), staging: None, area: None, area_age: 0 })
        }
    }

    /// The window's inside within a captured picture of tw × th.
    fn inside(&mut self, tw: u32, th: u32) -> (u32, u32, u32, u32) {
        self.area_age += 1;
        if let Some(a) = self.area {
            if self.area_age < 60 && a.0 + a.2 <= tw && a.1 + a.3 <= th {
                return a;
            }
        }
        self.area_age = 0;
        let whole = (0, 0, tw, th);
        // SAFETY: plain queries on the window handle; a closed window just fails.
        let found = unsafe {
            let mut frame = RECT::default();
            let mut client = RECT::default();
            let mut origin = POINT { x: 0, y: 0 };
            if DwmGetWindowAttribute(self.hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut RECT as *mut core::ffi::c_void, std::mem::size_of::<RECT>() as u32).is_err() || GetClientRect(self.hwnd, &mut client).is_err() {
                None
            } else {
                let _ = ClientToScreen(self.hwnd, &mut origin);
                let x0 = (origin.x - frame.left).clamp(0, tw as i32) as u32;
                let y0 = (origin.y - frame.top).clamp(0, th as i32) as u32;
                let cw = ((client.right - client.left).max(0) as u32).min(tw - x0);
                let ch = ((client.bottom - client.top).max(0) as u32).min(th - y0);
                (cw >= 16 && ch >= 16).then_some((x0, y0, cw, ch))
            }
        };
        self.area = Some(found.unwrap_or(whole));
        self.area.unwrap()
    }

    /// The next picture (BGRA, the window's inside), or None when no new one has come yet.
    pub fn next(&mut self) -> Result<Option<(usize, usize, Vec<u8>)>, String> {
        let Ok(frame) = self.pool.TryGetNextFrame() else { return Ok(None) };
        let content = frame.ContentSize().map_err(why)?;
        if (content.Width, content.Height) != self.size {
            let _ = frame.Close();
            self.pool.Recreate(&self.d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content).map_err(why)?;
            self.size = (content.Width, content.Height);
            self.area = None;
            return Ok(None);
        }
        // SAFETY: Direct3D calls on objects owned here; the staging texture is mapped and unmapped
        // within this block and the rows are copied out before unmapping.
        let out = unsafe {
            let surface = frame.Surface().map_err(why)?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(why)?;
            let texture: ID3D11Texture2D = access.GetInterface().map_err(why)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let (x0, y0, cw, ch) = self.inside(desc.Width, desc.Height);
            if self.staging.as_ref().is_none_or(|s| s.1 != cw || s.2 != ch) {
                let sd = D3D11_TEXTURE2D_DESC {
                    Width: cw,
                    Height: ch,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut tex = None;
                self.device.CreateTexture2D(&sd, None, Some(&mut tex)).map_err(why)?;
                self.staging = Some((tex.ok_or("no staging texture")?, cw, ch));
            }
            let staging = &self.staging.as_ref().unwrap().0;
            let area = D3D11_BOX { left: x0, top: y0, front: 0, right: x0 + cw, bottom: y0 + ch, back: 1 };
            self.context.CopySubresourceRegion(staging, 0, 0, 0, 0, &texture, 0, Some(&area));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(why)?;
            let (w, h, pitch) = (cw as usize, ch as usize, mapped.RowPitch as usize);
            let mut out = vec![0u8; w * h * 4];
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * h);
            for y in 0..h {
                out[y * w * 4..(y + 1) * w * 4].copy_from_slice(&src[y * pitch..y * pitch + w * 4]);
            }
            self.context.Unmap(staging, 0);
            (w, h, out)
        };
        let _ = frame.Close();
        Ok(Some(out))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
