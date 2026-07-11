use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Position {
    pub x: i64,
    pub y: i64,
}

pub fn room_at_position(position: Position) -> Option<&'static str> {
    for (room, left, right, top, bottom) in [
        ("power", 1, 4, 1, 3),
        ("junction", 5, 7, 1, 3),
        ("oxygen", 8, 12, 1, 3),
        ("diagnostics", 1, 4, 5, 7),
        ("charger", 5, 7, 5, 7),
        ("valve", 8, 12, 5, 7),
        ("airlock", 5, 7, 8, 9),
        ("signal", 8, 12, 8, 9),
    ] {
        if position.x >= left && position.x <= right && position.y >= top && position.y <= bottom {
            return Some(room);
        }
    }
    for (room, x, y) in [
        ("junction", 6, 4),
        ("oxygen", 10, 4),
        ("diagnostics", 2, 4),
        ("charger", 6, 4),
        ("valve", 10, 4),
    ] {
        if position.x == x && position.y == y {
            return Some(room);
        }
    }
    None
}

pub fn is_walkable(position: Position) -> bool {
    room_at_position(position).is_some()
}

pub fn room_name(room: &str) -> &'static str {
    match room {
        "power" => "Power Bay",
        "junction" => "Junction",
        "oxygen" => "Oxygen Nook",
        "diagnostics" => "Diagnostics",
        "charger" => "Battery Charger",
        "valve" => "Valve Room",
        "signal" => "Signal Array",
        "airlock" => "Airlock",
        _ => "Junction",
    }
}

pub fn room_exits(room: &str) -> &'static [&'static str] {
    match room {
        "power" => &["junction", "charger"],
        "junction" => &["power", "oxygen", "diagnostics", "airlock"],
        "oxygen" => &["junction", "valve"],
        "valve" => &["oxygen", "charger", "signal"],
        "charger" => &["power", "valve", "signal"],
        "signal" => &["valve", "charger", "airlock"],
        "airlock" => &["junction", "signal"],
        "diagnostics" => &["junction"],
        _ => &[],
    }
}

pub fn room_center(room: &str) -> Position {
    match room {
        "power" => Position { x: 2, y: 2 },
        "junction" => Position { x: 6, y: 2 },
        "oxygen" => Position { x: 10, y: 2 },
        "diagnostics" => Position { x: 2, y: 6 },
        "charger" => Position { x: 6, y: 6 },
        "valve" => Position { x: 10, y: 6 },
        "signal" => Position { x: 10, y: 8 },
        "airlock" => Position { x: 6, y: 8 },
        _ => Position { x: 6, y: 2 },
    }
}

pub fn door_kind_for_step(source: Position, target: Position) -> Option<&'static str> {
    for (a, b, kind) in [
        (Position { x: 4, y: 2 }, Position { x: 5, y: 2 }, "open"),
        (Position { x: 7, y: 2 }, Position { x: 8, y: 2 }, "open"),
        (Position { x: 6, y: 3 }, Position { x: 6, y: 4 }, "open"),
        (Position { x: 6, y: 4 }, Position { x: 6, y: 5 }, "open"),
        (Position { x: 10, y: 3 }, Position { x: 10, y: 4 }, "open"),
        (Position { x: 10, y: 4 }, Position { x: 10, y: 5 }, "open"),
        (Position { x: 7, y: 6 }, Position { x: 8, y: 6 }, "open"),
        (Position { x: 6, y: 7 }, Position { x: 6, y: 8 }, "pressure"),
        (Position { x: 10, y: 7 }, Position { x: 10, y: 8 }, "open"),
        (Position { x: 7, y: 8 }, Position { x: 8, y: 8 }, "open"),
        (Position { x: 2, y: 3 }, Position { x: 2, y: 4 }, "open"),
        (Position { x: 2, y: 4 }, Position { x: 2, y: 5 }, "open"),
    ] {
        if (source == a && target == b) || (source == b && target == a) {
            return Some(kind);
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: &'static str,
}

pub fn devices_at_position(position: Position, battery_location: &str) -> Vec<Device> {
    let mut devices = vec![];
    for (id, x, y) in [
        ("fuse-blue", 1, 1),
        ("fuse-yellow", 2, 1),
        ("fuse-red", 3, 1),
        ("breaker-main", 1, 3),
        ("breaker-aux", 2, 3),
        ("bypass", 5, 2),
        ("plate", 7, 2),
        ("charger", 5, 5),
        ("battery", 6, 6),
        ("valve-a", 8, 5),
        ("valve-c", 9, 5),
        ("valve-floodgate", 11, 5),
        ("relay", 9, 8),
        ("beacon", 6, 8),
    ] {
        if position.x == x && position.y == y && (id != "battery" || battery_location == "charger") {
            devices.push(Device { id });
        }
    }
    if battery_location == "signal" && position.x == 8 && position.y == 8 {
        devices.push(Device { id: "battery" });
    }
    devices
}
