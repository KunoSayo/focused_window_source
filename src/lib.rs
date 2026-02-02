mod active_window_manager;

use crate::active_window_manager::update_active;
use active_win_pos_rs::ActiveWindow;
use anyhow::Result;
use libc::c_int;
use libobs::*;
use std::ffi::{CStr, CString, OsStr};
use std::mem::zeroed;
use std::ops::Deref;
use std::os::raw::c_void;
use std::ptr::null_mut;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

// 全局任务队列
static TICK_TASKS: Mutex<Vec<FocusedWindowSource>> = Mutex::new(Vec::new());

// 模块指针
#[unsafe(no_mangle)]
static mut OBS_MODULE_POINTER: *mut obs_module_info = null_mut();

// 设置模块指针
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_module_set_pointer(module: *mut obs_module_info) {
    unsafe {
        OBS_MODULE_POINTER = module;
    }
}

// 获取当前模块
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_current_module() -> *mut obs_module_info {
    unsafe { OBS_MODULE_POINTER }
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_ver() -> u32 {
    0
}

// 模块信息
#[unsafe(no_mangle)]
pub static mut OBS_MODULE_INFO: obs_module_info = obs_module_info {
    bin_path: std::ptr::null(),
    data_path: std::ptr::null(),
    ..unsafe { zeroed() }
};

// 定义匹配方法枚举
#[derive(Debug, Clone, Copy, PartialEq)]
enum MatchMethod {
    Title,       // 只匹配标题
    Application, // 只匹配应用程序类型
    Strict,      // 严格匹配：应用程序类型+标题
}

impl MatchMethod {
    fn from_str(s: &str) -> Self {
        match s {
            "application" => MatchMethod::Application,
            "strict" => MatchMethod::Strict,
            _ => MatchMethod::Title,
        }
    }
}

// 定义Focused Window Source结构体
#[derive(Clone)]
struct FocusedWindowSource {
    scene_name: Arc<Mutex<String>>,
    scene_item_list: Arc<Mutex<Vec<String>>>,
    enable: Arc<AtomicBool>,
    match_method: Arc<Mutex<MatchMethod>>,
}

impl FocusedWindowSource {
    fn new() -> Result<Self> {
        Ok(Self {
            scene_name: Arc::new(Mutex::new("".to_string())),
            scene_item_list: Arc::new(Mutex::new(Vec::new())),
            enable: Default::default(),
            match_method: Arc::new(Mutex::new(MatchMethod::Title)),
        })
    }

    fn update_scene_list(&self, scene_list: Vec<String>) {
        let mut scenes = self.scene_item_list.lock().unwrap();
        *scenes = scene_list;
    }

    // 检查窗口标题是否匹配（支持部分匹配）
    fn is_str_matched(&self, title1: &str, title2: &str) -> bool {
        if title1.is_empty() || title2.is_empty() {
            return false;
        }
        // 完全匹配
        if title1 == title2 {
            return true;
        }

        // 检查是否一个标题是另一个的子字符串
        if title1.contains(title2) || title2.contains(title1) {
            return true;
        }

        // 移除常见的后缀进行比较
        let clean_title1 = title1.trim_end_matches(" - ").trim_end_matches(" | ");
        let clean_title2 = title2.trim_end_matches(" - ").trim_end_matches(" | ");

        clean_title1 == clean_title2
    }

    // 根据选择的匹配方法检查窗口是否匹配
    fn is_window_matched_with_method(
        &self,
        focused_window: &ActiveWindow,
        source_name: &str,
    ) -> bool {
        let match_method = self.match_method.lock().unwrap();

        match *match_method {
            MatchMethod::Title => self.is_str_matched(&focused_window.title, source_name),
            MatchMethod::Application => self.is_str_matched(
                &focused_window
                    .process_path
                    .file_name()
                    .unwrap_or(OsStr::new(""))
                    .to_string_lossy(),
                source_name,
            ),
            MatchMethod::Strict => {
                self.is_str_matched(
                    &focused_window
                        .process_path
                        .file_name()
                        .unwrap_or(OsStr::new(""))
                        .to_string_lossy(),
                    source_name,
                ) && self.is_str_matched(&focused_window.title, source_name)
            }
        }
    }

    // 根据场景名称查找场景源
    fn get_scene_item(&self, scene_item_name: &str) -> *mut obs_sceneitem_t {
        unsafe {
            // 使用 OBS API 查找场景

            let c_str = match CString::from_str(self.scene_name.lock().unwrap().as_str()) {
                Ok(x) => x,
                Err(_) => return null_mut(),
            };
            // 先通过名字查找 source
            let scene_source = obs_get_source_by_name(c_str.as_ptr());
            if scene_source.is_null() {
                return null_mut();
            }

            if !obs_source_is_scene(scene_source) {
                obs_source_release(scene_source);
                return null_mut();
            }

            // 转换成 obs_scene_t
            let scene = obs_scene_from_source(scene_source);

            // 注意：scene_source 引用计数要释放
            let scene_item = obs_scene_find_source(scene, scene_item_name.as_ptr() as *const i8);
            obs_source_release(scene_source);

            scene_item
        }
    }
}

// 外部C函数
#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_name(_data: *mut c_void) -> *const i8 {
    b"Focused Window Source\0".as_ptr() as *const i8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn create(
    settings: *mut obs_data_t,
    _source: *mut obs_source_t,
) -> *mut c_void {
    if let Ok(instance) = FocusedWindowSource::new() {
        let instance = Box::into_raw(Box::new(instance));

        let ret = instance as *mut c_void;
        unsafe {
            update(ret, settings);
        }
        ret
    } else {
        null_mut()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn destroy(data: *mut c_void) {
    if !data.is_null() {
        unsafe {
            let _ = Box::from_raw(data as *mut FocusedWindowSource);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn update(data: *mut c_void, settings: *mut obs_data_t) {
    if !data.is_null() {
        unsafe {
            let instance = &mut *(data as *mut FocusedWindowSource);
            // 从设置中获取场景列表
            if let Ok(scene_names) = instance.get_scene_names_from_settings(settings) {
                instance.update_scene_list(scene_names);
                *instance.scene_name.lock().unwrap() =
                    CStr::from_ptr(obs_data_get_string(settings, b"scene\0".as_ptr() as _))
                        .to_string_lossy()
                        .to_string();

                // 从设置中获取匹配方法
                let match_method_str = CStr::from_ptr(obs_data_get_string(
                    settings,
                    b"match_method\0".as_ptr() as _,
                ))
                .to_string_lossy()
                .to_string();
                *instance.match_method.lock().unwrap() = MatchMethod::from_str(&match_method_str);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn video_render(_data: *mut c_void, _effect: *mut gs_effect_t) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_width(_data: *mut c_void) -> u32 {
    99
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_height(_data: *mut c_void) -> u32 {
    99
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_defaults(_settings: *mut obs_data_t) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_properties(data: *mut c_void) -> *mut obs_properties_t {
    if !data.is_null() {
        unsafe {
            let props = obs_properties_create();

            // 添加场景列表属性（可编辑列表）
            let _ = obs_properties_add_editable_list(
                props,
                b"items\0".as_ptr() as *const i8,
                b"Items\0".as_ptr() as *const i8,
                obs_editable_list_type_OBS_EDITABLE_LIST_TYPE_STRINGS,
                null_mut(),
                null_mut(),
            );

            obs_properties_add_text(
                props,
                b"scene\0".as_ptr() as *const i8,
                b"Scene\0".as_ptr() as *const i8,
                0, // OBS_TEXT_DEFAULT
            );

            // 添加匹配方法选项
            let match_method_list = obs_properties_add_list(
                props,
                b"match_method\0".as_ptr() as *const i8,
                b"Match Method\0".as_ptr() as *const i8,
                obs_combo_type_OBS_COMBO_TYPE_LIST,
                obs_combo_format_OBS_COMBO_FORMAT_STRING,
            );

            obs_property_list_add_string(
                match_method_list,
                b"Title\0".as_ptr() as *const i8,
                b"title\0".as_ptr() as *const i8,
            );

            obs_property_list_add_string(
                match_method_list,
                b"Application\0".as_ptr() as *const i8,
                b"application\0".as_ptr() as *const i8,
            );

            obs_property_list_add_string(
                match_method_list,
                b"Strict (App + Title)\0".as_ptr() as *const i8,
                b"strict\0".as_ptr() as *const i8,
            );

            props
        }
    } else {
        null_mut()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn activate(data: *mut c_void) {
    if !data.is_null() {
        unsafe {
            let instance = &mut *(data as *mut FocusedWindowSource);
            instance.enable.store(true, Ordering::Relaxed);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn deactivate(data: *mut c_void) {
    if !data.is_null() {
        unsafe {
            let instance = &mut *(data as *mut FocusedWindowSource);
            instance.enable.store(false, Ordering::Relaxed);
        }
    }
}

// 执行主线程中的任务
#[unsafe(no_mangle)]
unsafe extern "C" fn execute_tick_tasks(_: *mut c_void) {
    if let Ok(mut tasks_guard) = TICK_TASKS.try_lock() {
        // 获取所有待执行的任务
        let tasks = &mut *tasks_guard;

        if let Ok(guard) = active_window_manager::ACTIVE_WINDOW.read() {
            if let Some(focused) = guard.deref().clone() {
                drop(guard);
                // 为每个任务执行逻辑
                for instance in tasks.iter() {
                    let scene_list = instance.scene_item_list.lock().unwrap();

                    let mut first_scene = null_mut();
                    let mut focused_scene = null_mut();
                    let mut first_one = c_int::MIN;

                    for scene_name in scene_list.iter() {
                        let scene = instance.get_scene_item(scene_name);
                        if !scene.is_null() {
                            let source = unsafe { obs_sceneitem_get_source(scene) };
                            if !source.is_null() {
                                let order = unsafe { obs_sceneitem_get_order_position(scene) };
                                if order >= first_one {
                                    first_one = order;
                                    first_scene = scene;
                                }
                                let settings = unsafe { obs_source_get_settings(source) };
                                let name = unsafe {
                                    obs_data_get_string(settings, b"window\0".as_ptr() as *const i8)
                                };
                                let that_title =
                                    unsafe { CStr::from_ptr(name).to_string_lossy().to_string() };
                                if instance.is_window_matched_with_method(&focused, &that_title) {
                                    focused_scene = scene;
                                }
                            }
                        }
                    }

                    if focused_scene != first_scene
                        && !focused_scene.is_null()
                        && !first_scene.is_null()
                    {
                        let focused_order =
                            unsafe { obs_sceneitem_get_order_position(focused_scene) };
                        unsafe {
                            obs_sceneitem_set_order_position(focused_scene, first_one);
                            obs_sceneitem_set_order_position(first_scene, focused_order);
                        }
                    }
                }
            }
        }
        tasks.clear();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn video_tick(data: *mut c_void, _seconds: f32) {
    if !data.is_null() {
        unsafe {
            let instance = &*(data as *const FocusedWindowSource);
            if !instance.enable.load(Ordering::Relaxed) {
                return;
            }

            // 更新活动窗口
            update_active();
            let mut tasks = TICK_TASKS.lock().unwrap();
            tasks.push(instance.clone());
            obs_queue_task(
                obs_task_type_OBS_TASK_UI,
                Some(execute_tick_tasks),
                null_mut(),
                false,
            );
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn load(data: *mut c_void, settings: *mut obs_data_t) {
    unsafe {
        update(data, settings);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn save(_data: *mut c_void, _settings: *mut obs_data_t) {}

// 从设置中获取场景列表
impl FocusedWindowSource {
    fn get_scene_names_from_settings(&self, settings: *mut obs_data_t) -> Result<Vec<String>> {
        unsafe {
            let scenes_array = obs_data_get_array(settings, b"items\0".as_ptr() as *const i8);
            if scenes_array.is_null() {
                return Ok(Vec::new());
            }
            obs_data_set_array(settings, b"items\0".as_ptr() as *const i8, scenes_array);
            let count = obs_data_array_count(scenes_array);
            let mut scene_names = Vec::new();

            for i in 0..count {
                let scene_data = obs_data_array_item(scenes_array, i);
                let scene_name = obs_data_get_string(scene_data, b"value\0".as_ptr() as *const i8);

                if !scene_name.is_null() {
                    let name = CStr::from_ptr(scene_name).to_string_lossy().into_owned();
                    scene_names.push(name);
                }
            }

            Ok(scene_names)
        }
    }
}

// 模块加载和卸载函数
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_module_load() -> bool {
    unsafe {
        // 注册源
        obs_register_source_s(&raw const OBS_SOURCE_INFO, size_of::<obs_source_info>());
    }
    active_window_manager::run_thread();
    true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_module_unload() -> bool {
    active_window_manager::stop_thread();
    true
}

// 返回模块的唯一ID
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_get_module_id(_module: *const obs_module_info) -> *const i8 {
    b"focused_window_source\0".as_ptr() as *const i8
}

// 导出源信息 - 只保留一个定义
#[unsafe(no_mangle)]
pub static mut OBS_SOURCE_INFO: obs_source_info = obs_source_info {
    id: b"focused_window_source\0".as_ptr() as *const i8,
    type_: obs_source_type_OBS_SOURCE_TYPE_INPUT as i32,
    output_flags: OBS_SOURCE_VIDEO,
    get_name: Some(get_name),
    create: Some(create),
    destroy: Some(destroy),
    update: Some(update),
    video_render: Some(video_render as unsafe extern "C" fn(*mut c_void, *mut gs_effect)),
    get_width: Some(get_width as unsafe extern "C" fn(*mut c_void) -> u32),
    get_height: Some(get_height as unsafe extern "C" fn(*mut c_void) -> u32),
    get_defaults: Some(get_defaults),
    get_properties: Some(get_properties),
    activate: Some(activate),
    deactivate: Some(deactivate),
    video_tick: Some(video_tick),
    ..unsafe { zeroed() }
};
