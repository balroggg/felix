use super::config::*;
use super::functions::*;
use super::nums::*;
use chrono::prelude::*;
use log::debug;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use termion::{clear, color, cursor, style};

pub const STARTING_POINT: u16 = 3;
pub const DOWN_ARROW: char = '\u{21D3}';
pub const RIGHT_ARROW: char = '\u{21D2}';
pub const FX_CONFIG_DIR: &str = "felix";
pub const CONFIG_FILE: &str = "config.toml";
pub const TRASH: &str = "trash";
pub const WHEN_EMPTY: &str = "Are you sure to empty the trash directory? (if yes: y)";

macro_rules! print_item {
    ($color: expr, $name: expr, $time: expr, $selected: expr, $time_start_pos: expr, $column: expr) => {
        if *($selected) {
            print!(
                "{}{}{}{}{}{} {}{}{}",
                $color,
                style::Invert,
                $name,
                style::Reset,
                cursor::Left(60),
                cursor::Right($time_start_pos),
                style::Invert,
                $time,
                style::Reset
            );
        } else {
            print!(
                "{}{}{}{} {}{}",
                $color,
                $name,
                cursor::Left(60),
                cursor::Right($time_start_pos),
                $time,
                color::Fg(color::Reset)
            );
        }
        if $column > 58 {
            print!("{}", (0..8).map(|_| ' ').collect::<String>());
        } else if $column > 49 {
            print!("{}", (0..($column - 49)).map(|_| ' ').collect::<String>());
        }
    };
}
#[derive(Clone)]
pub struct State {
    pub list: Vec<ItemInfo>,
    pub registered: Vec<ItemInfo>,
    pub current_dir: PathBuf,
    pub trash_dir: PathBuf,
    pub colors: (Colorname, Colorname, Colorname),
    pub default: String,
    pub commands: HashMap<String, String>,
    pub sort_by: SortKey,
    pub layout: Layout,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct ItemInfo {
    pub file_type: FileType,
    pub file_name: String,
    pub file_path: std::path::PathBuf,
    pub file_size: u64,
    pub file_ext: Option<OsString>,
    pub modified: Option<String>,
    pub selected: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileType {
    Directory,
    File,
    Symlink,
}

#[derive(Clone)]
pub struct Layout {
    pub terminal_row: u16,
    pub terminal_column: u16,
    pub name_max_len: usize,
    pub time_start_pos: u16,
}

impl Default for State {
    fn default() -> Self {
        let config = read_config().unwrap();
        State {
            list: Vec::new(),
            registered: Vec::new(),
            current_dir: PathBuf::new(),
            trash_dir: PathBuf::new(),
            colors: (
                config.color.dir_fg,
                config.color.file_fg,
                config.color.symlink_fg,
            ),
            default: config.default,
            commands: to_extension_map(&config.exec),
            sort_by: config.sort_by,
            layout: Layout {
                terminal_row: 0,
                terminal_column: 0,
                name_max_len: 0,
                time_start_pos: 0,
            },
        }
    }
}

impl State {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn get_item(&self, index: usize) -> Result<&ItemInfo, std::io::Error> {
        self.list
            .get(index)
            .ok_or_else(|| Error::new(ErrorKind::NotFound, "cannot choose item."))
    }

    pub fn open_file(&self, index: usize) -> std::io::Result<ExitStatus> {
        let item = self.get_item(index)?;
        let path = &item.file_path;
        let map = &self.commands;
        let extention = path.extension();

        match extention {
            Some(extention) => {
                let ext = extention.to_os_string().into_string().unwrap();
                match map.get(&ext) {
                    Some(command) => {
                        let mut ex = Command::new(command);
                        ex.arg(path).status()
                    }
                    None => {
                        let mut ex = Command::new(&self.default);
                        ex.arg(path).status()
                    }
                }
            }

            None => {
                let mut ex = Command::new(&self.default);
                ex.arg(path).status()
            }
        }
    }

    pub fn remove_and_yank_file(&mut self, item: ItemInfo) -> std::io::Result<()> {
        //prepare from and to for copy
        let from = &item.file_path;

        if item.file_type == FileType::Symlink && !from.exists() {
            let _ = Command::new("rm").arg(from).status();
            Ok(())
        } else {
            let name = &item.file_name;
            let mut rename = Local::now().timestamp().to_string();
            rename.push('_');
            rename.push_str(name);

            let to = self.trash_dir.join(&rename);

            //copy
            std::fs::copy(from, &to)?;

            self.to_registered_mut(&item, to, rename);

            //remove original
            std::fs::remove_file(from)?;

            Ok(())
        }
    }

    pub fn remove_and_yank_dir(&mut self, item: ItemInfo) -> std::io::Result<()> {
        let mut trash_name = String::new();
        let mut base: usize = 0;
        let mut trash_path: std::path::PathBuf = PathBuf::new();
        let mut target: PathBuf;

        let mut i = 0;
        for entry in walkdir::WalkDir::new(&item.file_path).sort_by_key(|x| x.path().to_path_buf())
        {
            let entry = entry?;
            if i == 0 {
                base = entry.path().iter().count();

                trash_name = chrono::Local::now().timestamp().to_string();
                trash_name.push('_');
                trash_name.push_str(entry.file_name().to_str().unwrap());
                trash_path = self.trash_dir.join(&trash_name);
                std::fs::create_dir(&self.trash_dir.join(&trash_path))?;

                i += 1;
                continue;
            } else {
                target = entry.path().iter().skip(base).collect();
                target = trash_path.join(target);
                if entry.file_type().is_dir() {
                    std::fs::create_dir(&target)?;
                    continue;
                }

                if let Some(parent) = entry.path().parent() {
                    if !parent.exists() {
                        std::fs::create_dir(parent)?;
                    }
                }

                std::fs::copy(entry.path(), &target)?;
            }
        }

        self.to_registered_mut(&item, trash_path, trash_name);

        //remove original
        std::fs::remove_dir_all(&item.file_path)?;

        Ok(())
    }

    fn to_registered_mut(&mut self, item: &ItemInfo, file_path: PathBuf, file_name: String) {
        let mut buf = item.clone();
        buf.file_path = file_path;
        buf.file_name = file_name;
        buf.selected = false;
        self.registered.push(buf);
    }

    pub fn yank_item(&mut self, index: usize, selected: bool) {
        self.registered.clear();
        if selected {
            for item in self.list.iter_mut().filter(|item| item.selected) {
                self.registered.push(item.clone());
            }
        } else {
            let item = self.get_item(index).unwrap().clone();
            self.registered.push(item);
        }
    }

    pub fn put_items(&mut self) -> std::io::Result<()> {
        //make HashSet<String> of file_name
        let mut name_set = HashSet::new();
        for item in self.list.iter() {
            name_set.insert(item.file_name.clone());
        }

        for item in self.registered.clone().into_iter() {
            match item.file_type {
                FileType::Directory => {
                    self.put_dir(&item, &mut name_set)?;
                }
                FileType::File | FileType::Symlink => {
                    self.put_file(&item, &mut name_set)?;
                }
            }
        }
        Ok(())
    }

    fn put_file(&mut self, item: &ItemInfo, name_set: &mut HashSet<String>) -> std::io::Result<()> {
        if item.file_path.parent() == Some(&self.trash_dir) {
            let mut item = item.clone();
            let rename = item.file_name.chars().skip(11).collect();
            item.file_name = rename;
            let rename = rename_file(&item, name_set);
            std::fs::copy(&item.file_path, &self.current_dir.join(&rename))?;
            name_set.insert(rename);
        } else {
            let rename = rename_file(item, name_set);
            std::fs::copy(&item.file_path, &self.current_dir.join(&rename))?;
            name_set.insert(rename);
        }
        Ok(())
    }

    fn put_dir(&mut self, buf: &ItemInfo, name_set: &mut HashSet<String>) -> std::io::Result<()> {
        let mut base: usize = 0;
        let mut target: PathBuf = PathBuf::new();
        let original_path = &(buf).file_path;

        let mut i = 0;
        for entry in walkdir::WalkDir::new(&original_path).sort_by_key(|x| x.path().to_path_buf()) {
            let entry = entry?;
            if i == 0 {
                base = entry.path().iter().count();

                let parent = &original_path.parent().unwrap();
                if parent == &self.trash_dir {
                    let mut buf = buf.clone();
                    let rename = buf.file_name.chars().skip(11).collect();
                    buf.file_name = rename;

                    let rename = rename_dir(&buf, name_set);
                    target = self.current_dir.join(&rename);
                    name_set.insert(rename);
                } else {
                    let rename = rename_dir(buf, name_set);
                    target = self.current_dir.join(&rename);
                    name_set.insert(rename);
                }
                std::fs::create_dir(&target)?;
                i += 1;
                continue;
            } else {
                let child: PathBuf = entry.path().iter().skip(base).collect();
                let child = target.join(child);

                if entry.file_type().is_dir() {
                    std::fs::create_dir(child)?;
                    continue;
                } else if let Some(parent) = entry.path().parent() {
                    if !parent.exists() {
                        std::fs::create_dir(parent)?;
                    }
                }

                std::fs::copy(entry.path(), &child)?;
            }
        }
        Ok(())
    }

    pub fn print(&self, index: usize) {
        let item = &self.get_item(index).unwrap();
        let chars: Vec<char> = item.file_name.chars().collect();
        let name = if chars.len() > self.layout.name_max_len {
            let mut result = chars
                .iter()
                .take(self.layout.name_max_len - 3)
                .collect::<String>();
            result.push_str("..");
            result
        } else {
            item.file_name.clone()
        };
        let time = format_time(&item.modified);
        let selected = &item.selected;
        let color = match item.file_type {
            FileType::Directory => &self.colors.0,
            FileType::File => &self.colors.1,
            FileType::Symlink => &self.colors.2,
        };
        match color {
            Colorname::AnsiValue(n) => {
                print_item!(
                    color::Fg(color::AnsiValue(*n)),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::Black => {
                print_item!(
                    color::Fg(color::Black),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::Blue => {
                print_item!(
                    color::Fg(color::Blue),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::Cyan => {
                print_item!(
                    color::Fg(color::Cyan),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::Green => {
                print_item!(
                    color::Fg(color::Green),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::LightBlack => {
                print_item!(
                    color::Fg(color::LightBlack),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::LightBlue => {
                print_item!(
                    color::Fg(color::LightBlue),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::LightCyan => {
                print_item!(
                    color::Fg(color::LightCyan),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::LightGreen => {
                print_item!(
                    color::Fg(color::LightGreen),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::LightMagenta => {
                print_item!(
                    color::Fg(color::LightMagenta),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::LightRed => {
                print_item!(
                    color::Fg(color::LightRed),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::LightWhite => {
                print_item!(
                    color::Fg(color::LightWhite),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::LightYellow => {
                print_item!(
                    color::Fg(color::LightYellow),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::Magenta => {
                print_item!(
                    color::Fg(color::Magenta),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::Red => {
                print_item!(
                    color::Fg(color::Red),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::Rgb(x, y, z) => {
                print_item!(
                    color::Fg(color::Rgb(*x, *y, *z)),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::White => {
                print_item!(
                    color::Fg(color::White),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
            Colorname::Yellow => {
                print_item!(
                    color::Fg(color::Yellow),
                    name,
                    time,
                    selected,
                    self.layout.time_start_pos,
                    self.layout.terminal_column
                );
            }
        }
    }

    pub fn list_up(&self, skip_number: u16) {
        debug!("list_up starts.");
        let row = self.layout.terminal_row;
        let len = self.list.len();

        //if list exceeds max-row
        if len > (row - STARTING_POINT) as usize - 1 {
            let mut row_count = 0;
            for (i, _) in self.list.iter().enumerate() {
                if i < skip_number as usize {
                    continue;
                }

                print!(
                    "{}",
                    cursor::Goto(3, i as u16 + STARTING_POINT - skip_number)
                );

                if row_count == row - STARTING_POINT {
                    print!(
                        "  {}{}{}lines {}-{}({}){}{}",
                        cursor::Left(2),
                        color::Bg(color::LightWhite),
                        color::Fg(color::Black),
                        skip_number + 1,
                        row - STARTING_POINT + skip_number,
                        len,
                        color::Bg(color::Reset),
                        color::Fg(color::Reset)
                    );
                    break;
                } else {
                    self.print(i);
                    row_count += 1;
                }
            }
        } else {
            for (i, _) in self.list.iter().enumerate() {
                print!("{}", cursor::Goto(3, i as u16 + STARTING_POINT));
                self.print(i);
            }
        }
    }

    pub fn update_list(&mut self) {
        self.list = push_items(&self.current_dir, &self.sort_by).unwrap();
    }

    pub fn reset_selection(&mut self) {
        for mut item in self.list.iter_mut() {
            item.selected = false;
        }
    }

    pub fn select_from_top(&mut self, start_pos: usize) {
        for (i, item) in self.list.iter_mut().enumerate() {
            if i <= start_pos {
                item.selected = true;
            } else {
                item.selected = false;
            }
        }
    }

    pub fn select_to_bottom(&mut self, start_pos: usize) {
        for (i, item) in self.list.iter_mut().enumerate() {
            if i < start_pos {
                item.selected = false;
            } else {
                item.selected = true;
            }
        }
    }

    pub fn move_cursor(&self, nums: &Num, y: u16) {
        print!("{}", cursor::Goto(1, self.layout.terminal_row));
        print!("{}", clear::CurrentLine);
        debug!("move_cursor setup finished.");

        let item = self.get_item(nums.index);
        debug!("get_item finished.");
        if let Ok(item) = item {
            match &item.file_ext {
                Some(ext) => {
                    print!(
                        "[{}/{}] {} {}",
                        nums.index + 1,
                        self.list.len(),
                        ext.clone().into_string().unwrap_or_default(),
                        to_proper_size(item.file_size)
                    );
                }
                None => {
                    debug!("no extensions arm starts.");
                    print!(
                        "[{}/{}] {}",
                        nums.index + 1,
                        self.list.len(),
                        to_proper_size(item.file_size)
                    );
                    debug!("no extensions arm finished.");
                }
            }
        }
        print!("{}>{}", cursor::Goto(1, y), cursor::Left(1));
        debug!("printing cursor finished.");
    }
}

fn make_item(dir: fs::DirEntry) -> ItemInfo {
    let path = dir.path();
    let metadata = &fs::symlink_metadata(&path);

    let time = match metadata {
        Ok(metadata) => {
            let sometime = metadata.modified().unwrap();
            let chrono_time: DateTime<Local> = DateTime::from(sometime);
            Some(chrono_time.to_rfc3339_opts(SecondsFormat::Secs, false))
        }
        Err(_) => None,
    };

    let filetype = match metadata {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_dir() {
                FileType::Directory
            } else if file_type.is_file() {
                FileType::File
            } else if file_type.is_symlink() {
                FileType::Symlink
            } else {
                FileType::File
            }
        }
        Err(_) => FileType::File,
    };

    let name = dir
        .file_name()
        .into_string()
        .unwrap_or_else(|_| panic!("failed to get file name."));

    let size = match metadata {
        Ok(metadata) => metadata.len(),
        Err(_) => 0,
    };

    let ext = path.extension().map(|s| s.to_os_string());

    ItemInfo {
        file_type: filetype,
        file_name: name,
        file_path: path,
        file_size: size,
        file_ext: ext,
        modified: time,
        selected: false,
    }
}

pub fn push_items(p: &Path, key: &SortKey) -> Result<Vec<ItemInfo>, Error> {
    let mut result = Vec::new();
    let mut dir_v = Vec::new();
    let mut file_v = Vec::new();

    for entry in fs::read_dir(p)? {
        let e = entry?;
        let entry = make_item(e);
        match entry.file_type {
            FileType::Directory => dir_v.push(entry),
            FileType::File | FileType::Symlink => file_v.push(entry),
        }
    }

    match key {
        SortKey::Name => {
            dir_v.sort_by(|a, b| natord::compare(&a.file_name, &b.file_name));
            file_v.sort_by(|a, b| natord::compare(&a.file_name, &b.file_name));
        }
        SortKey::Time => {
            dir_v.sort_by(|a, b| b.modified.partial_cmp(&a.modified).unwrap());
            file_v.sort_by(|a, b| b.modified.partial_cmp(&a.modified).unwrap());
        }
    }

    result.append(&mut dir_v);
    result.append(&mut file_v);
    Ok(result)
}
