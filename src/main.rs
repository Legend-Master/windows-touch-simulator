#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    ops::{Deref, DerefMut},
    sync::{LazyLock, Mutex},
    thread::sleep,
    time::Duration,
};
use windows::{
    core::{w, Owned},
    Win32::{
        Foundation::{
            GetLastError, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, LRESULT, POINT, WAIT_TIMEOUT,
            WPARAM,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE},
        },
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::{
                KeyboardAndMouse::{GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_RSHIFT},
                Pointer::{
                    InitializeTouchInjection, InjectTouchInput, POINTER_FLAGS, POINTER_FLAG_DOWN,
                    POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_UP,
                    POINTER_FLAG_UPDATE, POINTER_INFO, POINTER_TOUCH_INFO, TOUCH_FEEDBACK_DEFAULT,
                },
            },
            WindowsAndMessaging::{
                CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
                LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PT_TOUCH, WH_MOUSE_LL, WM_LBUTTONDOWN,
                WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
            },
        },
    },
};

macro_rules! log_error {
    ($expression: expr) => {
        if let Err(error) = $expression {
            println!("{error}");
        }
    };
    ($expression: expr, $context: literal) => {
        if let Err(error) = $expression {
            println!("Error while {}: {}", $context, error);
        }
    };
}

#[derive(Clone, Copy)]
enum AutoZooming {
    ZoomIn,
    ZoomOut,
}

static CURRENT_TOUCH_INFOS: Mutex<Vec<PointerTouchInfo>> = Mutex::new(Vec::new());
static KEEP_ALIVE_EVENT: LazyLock<WindowsEvent> = LazyLock::new(WindowsEvent::create);
static AUTO_ZOOMING_EVENT: LazyLock<WindowsEvent> = LazyLock::new(WindowsEvent::create);
static AUTO_ZOOMING: Mutex<Option<AutoZooming>> = Mutex::new(None);

fn main() {
    let _mutex_handle = unsafe {
        Owned::new(CreateMutexW(None, true, w!(r"Global\WindowsTouchSimulator")).unwrap())
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        panic!("An instance of Windows Touch Simulator is already running");
    }

    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).unwrap() };
    let hmod = unsafe { GetModuleHandleW(w!("kernel32.dll")).unwrap() };
    unsafe {
        SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(low_level_mouse_proc),
            Some(hmod.into()),
            0,
        )
        .unwrap()
    };

    unsafe { InitializeTouchInjection(2, TOUCH_FEEDBACK_DEFAULT).unwrap() }

    // Keep touch contacts alive
    std::thread::spawn(move || loop {
        unsafe { WaitForSingleObject(**KEEP_ALIVE_EVENT, INFINITE) };
        while unsafe { WaitForSingleObject(**KEEP_ALIVE_EVENT, 100) } == WAIT_TIMEOUT {
            let touch_infos = CURRENT_TOUCH_INFOS.lock().unwrap();
            if touch_infos.is_empty() {
                break;
            }
            unsafe {
                log_error!(
                    inject_touch_input(&touch_infos),
                    "sending touch move to keep the touch alive"
                )
            };
        }
    });

    // Perform auto zooming on event
    std::thread::spawn(move || loop {
        unsafe { WaitForSingleObject(**AUTO_ZOOMING_EVENT, INFINITE) };
        let Some(zoom_out) = *AUTO_ZOOMING.lock().unwrap() else {
            continue;
        };
        let mut touch_infos = CURRENT_TOUCH_INFOS.lock().unwrap();
        if touch_infos.is_empty() {
            continue;
        }
        for _ in 0..25 {
            sleep(Duration::from_millis(1));
            let first_contact = touch_infos.first_mut().unwrap();
            match zoom_out {
                AutoZooming::ZoomIn => {
                    first_contact.pointerInfo.ptPixelLocation.x -= 3;
                    first_contact.pointerInfo.ptPixelLocation.y -= 3;
                }
                AutoZooming::ZoomOut => {
                    first_contact.pointerInfo.ptPixelLocation.x += 3;
                    first_contact.pointerInfo.ptPixelLocation.y += 3;
                }
            }
            let second_contact = touch_infos.last_mut().unwrap();
            match zoom_out {
                AutoZooming::ZoomIn => {
                    second_contact.pointerInfo.ptPixelLocation.x += 3;
                    second_contact.pointerInfo.ptPixelLocation.y += 3;
                }
                AutoZooming::ZoomOut => {
                    second_contact.pointerInfo.ptPixelLocation.x -= 3;
                    second_contact.pointerInfo.ptPixelLocation.y -= 3;
                }
            }
            if let Err(error) = unsafe { inject_touch_input(&touch_infos) } {
                println!("Error while performing auto zoom: {error}");
                break;
            }
        }
        sleep(Duration::from_millis(5));
        for touch_info in touch_infos.iter_mut() {
            touch_info.pointerInfo.pointerFlags = POINTER_FLAG_UP;
        }
        unsafe {
            log_error!(
                inject_touch_input(&touch_infos),
                "sending touch end in auto zooming"
            )
        };
        touch_infos.clear();
        AUTO_ZOOMING.lock().unwrap().take();
    });

    #[cfg(feature = "system-tray")]
    let _system_tray = create_system_tray();

    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0).as_bool() } {
        unsafe {
            let _ = TranslateMessage(&message);
        };
        unsafe { DispatchMessageW(&message) };
    }
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code < 0 || AUTO_ZOOMING.lock().unwrap().is_some() {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let info = *(lparam.0 as *const MSLLHOOKSTRUCT);
    // dbg!(&wparam);
    // dbg!(&info);
    if info.flags & LLMHF_INJECTED != 0 {
        println!("injected, info.flags: {}", info.flags);
        return CallNextHookEx(None, code, wparam, lparam);
    }
    match wparam.0 as u32 {
        WM_LBUTTONDOWN => {
            if is_key_down(VK_RSHIFT) {
                println!("sending touch down");
                let touch_info = create_touch_info(
                    &info.pt,
                    POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                );
                let mut touch_infos = vec![touch_info];
                if is_key_down(VK_CONTROL) {
                    let mut second_contact = touch_info;
                    second_contact.pointerInfo.pointerId = 1;
                    touch_infos.push(second_contact);
                };
                let result = InjectTouchInput(&touch_infos);
                if result.is_ok() {
                    for touch_info in touch_infos.iter_mut() {
                        touch_info.pointerInfo.pointerFlags =
                            POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
                    }
                    *CURRENT_TOUCH_INFOS.lock().unwrap() = map_pointer_touch_info(touch_infos);
                    SetEvent(**KEEP_ALIVE_EVENT).unwrap();
                } else {
                    println!("Error while sending touch down: {result:?}");
                }
                return LRESULT(1);
            }
        }
        WM_MOUSEMOVE => {
            let mut touch_infos = CURRENT_TOUCH_INFOS.lock().unwrap();
            if let Some(touch_info) = touch_infos.last_mut() {
                touch_info.pointerInfo.ptPixelLocation = info.pt;
                log_error!(inject_touch_input(&touch_infos), "sending touch move");
            }
        }
        WM_LBUTTONUP => {
            let mut touch_infos = CURRENT_TOUCH_INFOS.lock().unwrap();
            if !touch_infos.is_empty() {
                println!("sending touch up");
                for touch_info in touch_infos.iter_mut() {
                    touch_info.pointerInfo.pointerFlags = POINTER_FLAG_UP;
                }
                log_error!(inject_touch_input(&touch_infos), "sending touch end");
                touch_infos.clear();
                SetEvent(**KEEP_ALIVE_EVENT).unwrap();
                return LRESULT(1);
            }
        }
        WM_MOUSEWHEEL => {
            if is_key_down(VK_RSHIFT) && { CURRENT_TOUCH_INFOS.lock().unwrap().is_empty() } {
                let zoom_out = HIWORD(info.mouseData) < 0;
                let mut first_contact = create_touch_info(
                    &info.pt,
                    POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                );
                let mut second_contact = first_contact;
                second_contact.pointerInfo.pointerId = 1;
                if zoom_out {
                    first_contact.pointerInfo.ptPixelLocation.x -= 100;
                    first_contact.pointerInfo.ptPixelLocation.y -= 100;
                    second_contact.pointerInfo.ptPixelLocation.x += 100;
                    second_contact.pointerInfo.ptPixelLocation.y += 100;
                }
                let mut touch_infos = vec![first_contact, second_contact];
                let result = InjectTouchInput(&touch_infos);
                match result {
                    Ok(_) => {
                        for touch_info in touch_infos.iter_mut() {
                            touch_info.pointerInfo.pointerFlags =
                                POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
                        }
                        *CURRENT_TOUCH_INFOS.lock().unwrap() = map_pointer_touch_info(touch_infos);
                        AUTO_ZOOMING.lock().unwrap().replace(if zoom_out {
                            AutoZooming::ZoomOut
                        } else {
                            AutoZooming::ZoomIn
                        });
                        SetEvent(**AUTO_ZOOMING_EVENT).unwrap();
                        return LRESULT(1);
                    }
                    Err(error) => {
                        println!("Error while sending touch down to start auto zooming: {error}");
                    }
                }
            }
        }
        _ => {}
    };

    CallNextHookEx(None, code, wparam, lparam)
}

fn map_pointer_touch_info(contacts: Vec<POINTER_TOUCH_INFO>) -> Vec<PointerTouchInfo> {
    contacts.into_iter().map(PointerTouchInfo).collect()
}

unsafe fn inject_touch_input(contacts: &[PointerTouchInfo]) -> windows::core::Result<()> {
    let contacts: Vec<_> = contacts.iter().map(|contact| **contact).collect();
    InjectTouchInput(&contacts)
}

fn create_touch_info(point: &POINT, flags: POINTER_FLAGS) -> POINTER_TOUCH_INFO {
    POINTER_TOUCH_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_TOUCH,
            ptPixelLocation: *point,
            pointerFlags: flags,
            // pointerId: 1,
            // dwTime: info.time,
            ..Default::default()
        },
        // rcContact: RECT {
        //     left: point.x,
        //     right: point.x,
        //     top: point.y,
        //     bottom: point.y,
        // },
        // touchMask: TOUCH_MASK_CONTACTAREA,
        ..Default::default()
    }
}

fn is_key_down(key: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(key.0.into()) & 0x8000u16 as i16 != 0 }
}

#[allow(non_snake_case)]
pub fn HIWORD(l: u32) -> i16 {
    ((l >> 16) & 0xffff) as _
}

/// Wraper around event handle from [`CreateEventW`] for making it [`Send`] and [`Sync`]
struct WindowsEvent(Owned<HANDLE>);

impl WindowsEvent {
    fn create() -> Self {
        Self(unsafe { Owned::new(CreateEventW(None, false, false, None).unwrap()) })
    }
}

unsafe impl Send for WindowsEvent {}
unsafe impl Sync for WindowsEvent {}

impl Deref for WindowsEvent {
    type Target = HANDLE;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Wraper around [`POINTER_TOUCH_INFO`] for making it [`Send`] and [`Sync`]
struct PointerTouchInfo(POINTER_TOUCH_INFO);

unsafe impl Send for PointerTouchInfo {}
unsafe impl Sync for PointerTouchInfo {}

impl Deref for PointerTouchInfo {
    type Target = POINTER_TOUCH_INFO;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for PointerTouchInfo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Creates a system tray for indicating the app state and a way to exit it
///
/// Panics on fail
#[cfg(feature = "system-tray")]
#[must_use = "Not using it will drop the TrayIcon and it will get removed immediately making it useless"]
fn create_system_tray() -> tray_icon::TrayIcon {
    use windows::Win32::UI::WindowsAndMessaging::IDI_APPLICATION;

    let menu = tray_icon::menu::Menu::new();
    menu.append(&tray_icon::menu::PredefinedMenuItem::quit(None))
        .unwrap();
    tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Windows Touch Simulator")
        .with_icon(tray_icon::Icon::from_resource(IDI_APPLICATION.0 as _, None).unwrap())
        .build()
        .unwrap()
}
