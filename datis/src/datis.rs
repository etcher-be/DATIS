use std::str::FromStr;
use std::thread;

use crate::error::Error;
use crate::station::{Airfield, AtisStation, Position, StaticWind};
use hlua51::{Lua, LuaFunction, LuaTable};
use regex::Regex;

pub struct Datis {
    pub stations: Vec<AtisStation>,
}

impl Datis {
    pub fn create(mut lua: Lua<'_>) -> Result<Self, Error> {
        debug!("Extracting ATIS stations from Mission Situation");

        let cpath = {
            let mut package: LuaTable<_> = get!(lua, "package")?;
            let cpath: String = get!(package, "cpath")?;
            cpath
        };

        let mut stations = {
            let mut dcs: LuaTable<_> = get!(lua, "DCS")?;

            let mut get_mission_description: LuaFunction<_> = get!(dcs, "getMissionDescription")?;
            let mission_situation: String = get_mission_description.call()?;

            extract_atis_stations(&mission_situation)
        };

        debug!("Detected ATIS Stations:");
        for station in &stations {
            debug!("  - {} (Freq: {})", station.name, station.atis_freq);
        }

        // FETCH AIRDROMES
        {
            let mut terrain: LuaTable<_> = get!(lua, "Terrain")?;

            let mut get_terrain_config: LuaFunction<_> = get!(terrain, "GetTerrainConfig")?;
            let mut airdromes: LuaTable<_> = get_terrain_config.call_with_args("Airdromes")?;

            let mut i = 12;
            while let Some(mut airdrome) = airdromes.get::<LuaTable<_>, _, _>(i) {
                i += 1;

                let id: String = get!(airdrome, "id")?;
                let display_name: String = get!(airdrome, "display_name")?;

                for station in stations.iter_mut() {
                    if station.name != id && station.name != display_name {
                        continue;
                    }

                    station.name = display_name.to_string();

                    let (x, y) = {
                        let mut reference_point: LuaTable<_> = get!(airdrome, "reference_point")?;
                        let x: f64 = get!(reference_point, "x")?;
                        let y: f64 = get!(reference_point, "y")?;
                        (x, y)
                    };

                    let alt = {
                        let mut default_camera_position: LuaTable<_> =
                            get!(airdrome, "default_camera_position")?;
                        let mut pnt: LuaTable<_> = get!(default_camera_position, "pnt")?;
                        let alt: f64 = get!(pnt, 2)?;
                        // This is only the alt of the camera position of the airfield, which seems to be
                        // usually elevated by about 100. Keep the 100 elevation above the ground
                        // as a sender position (for SRS LOS).
                        alt
                    };

                    let mut rwys: Vec<String> = Vec::new();

                    let mut runways: LuaTable<_> = get!(airdrome, "runways")?;
                    let mut j = 0;
                    while let Some(mut runway) = runways.get::<LuaTable<_>, _, _>(j) {
                        j += 1;
                        let start: String = get!(runway, "start")?;
                        let end: String = get!(runway, "end")?;
                        rwys.push(start);
                        rwys.push(end);
                    }

                    station.airfield = Some(Airfield {
                        position: Position { x, y, alt },
                        runways: rwys,
                    });

                    break;
                }
            }
        }

        stations.retain(|s| s.airfield.is_some());

        // get _current_mission.mission.weather
        let mut current_mission: LuaTable<_> = get!(lua, "_current_mission")?;
        let mut mission: LuaTable<_> = get!(current_mission, "mission")?;
        let mut weather: LuaTable<_> = get!(mission, "weather")?;

        // get atmosphere_type
        let atmosphere_type: f64 = get!(weather, "atmosphere_type")?;

        if atmosphere_type == 0.0 {
            // is static DCS weather system
            // get wind
            let mut wind: LuaTable<_> = get!(weather, "wind")?;
            let mut wind_at_ground: LuaTable<_> = get!(wind, "atGround")?;

            // get wind_at_ground.speed
            let wind_speed: f64 = get!(wind_at_ground, "speed")?;

            // get wind_at_ground.dir
            let mut wind_dir: f64 = get!(wind_at_ground, "dir")?;

            for station in stations.iter_mut() {
                // rotate dir
                wind_dir -= 180.0;
                if wind_dir < 0.0 {
                    wind_dir += 360.0;
                }

                station.static_wind = Some(StaticWind {
                    dir: wind_dir.to_radians(),
                    speed: wind_speed,
                });
            }
        }

        debug!("Valid ATIS Stations:");
        for station in &stations {
            debug!("  - {} (Freq: {})", station.name, station.atis_freq);
        }

        for station in stations {
            let cpath = cpath.clone();
            thread::spawn(move || {
                if let Err(err) = crate::srs::start(cpath, station) {
                    error!("Error starting SRS broadcast: {}", err);
                }
            });
        }

        Ok(Datis {
            stations: Vec::new(),
        })
    }
}

fn extract_atis_stations(situation: &str) -> Vec<AtisStation> {
    // extract ATIS stations and frequencies
    let re = Regex::new(r"ATIS ([a-zA-Z-]+) ([1-3]\d{2}(\.\d{1,3})?)").unwrap();
    let mut stations: Vec<AtisStation> = re
        .captures_iter(situation)
        .map(|caps| {
            let name = caps.get(1).unwrap().as_str().to_string();
            let freq = caps.get(2).unwrap().as_str();
            let freq = (f32::from_str(freq).unwrap() * 1_000_000.0) as u64;
            AtisStation {
                name,
                atis_freq: freq,
                traffic_freq: None,
                airfield: None,
                static_wind: None,
            }
        })
        .collect();

    // extract optional traffic frquencies
    let re = Regex::new(r"TRAFFIC ([a-zA-Z-]+) ([1-3]\d{2}(\.\d{1,3})?)").unwrap();
    for caps in re.captures_iter(situation) {
        let name = caps.get(1).unwrap().as_str().to_string();
        let freq = caps.get(2).unwrap().as_str();
        let freq = (f32::from_str(freq).unwrap() * 1_000_000.0) as u64;

        if let Some(station) = stations.iter_mut().find(|s| s.name == name) {
            station.traffic_freq = Some(freq);
        }
    }

    stations
}

#[cfg(test)]
mod test {
    use super::{extract_atis_stations, AtisStation};

    #[test]
    fn test_atis_extraction() {
        let stations = extract_atis_stations(
            r#"
            ATIS Kutaisi 251.000
            ATIS Batumi 131.5
            ATIS Senaki-Kolkhi 145

            TRAFFIC Batumi 255.00
        "#,
        );

        assert_eq!(
            stations,
            vec![
                AtisStation {
                    name: "Kutaisi".to_string(),
                    atis_freq: 251_000_000,
                    traffic_freq: None,
                    airfield: None,
                    static_wind: None,
                },
                AtisStation {
                    name: "Batumi".to_string(),
                    atis_freq: 131_500_000,
                    traffic_freq: Some(255_000_000),
                    airfield: None,
                    static_wind: None,
                },
                AtisStation {
                    name: "Senaki-Kolkhi".to_string(),
                    atis_freq: 145_000_000,
                    traffic_freq: None,
                    airfield: None,
                    static_wind: None,
                }
            ]
        );
    }
}
