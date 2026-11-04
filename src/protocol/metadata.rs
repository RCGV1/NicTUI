use super::types::{SettingMetadata, SettingType};

// Numeric bounds are stored integers; setting_value handles physical-unit conversion.
pub const SETTINGS_METADATA: &[SettingMetadata] = &[
    SettingMetadata {
        menu_num: "00",
        name: "Squelch",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 9,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "01",
        name: "Dual Watch",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "02",
        name: "Auto Floor",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "03",
        name: "Active VFO",
        setting_type: SettingType::Enum(&["VFO A", "VFO B"]),
    },
    SettingMetadata {
        menu_num: "04",
        name: "Step",
        setting_type: SettingType::Numeric {
            min: 1,
            max: 50000,
            unit: "kHz",
        },
    },
    SettingMetadata {
        menu_num: "05",
        name: "RX Filter Trans",
        setting_type: SettingType::Numeric {
            min: 1500,
            max: 4000,
            unit: "MHz",
        },
    },
    SettingMetadata {
        menu_num: "06",
        name: "TX Filter Trans",
        setting_type: SettingType::Numeric {
            min: 1500,
            max: 4000,
            unit: "MHz",
        },
    },
    SettingMetadata {
        menu_num: "07",
        name: "PTT Mode",
        setting_type: SettingType::Enum(&["Dual", "Single", "Hybrid"]),
    },
    SettingMetadata {
        menu_num: "08",
        name: "TX Mod Meter",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "09",
        name: "Mic Gain",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 31,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "10",
        name: "TX Deviation",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 99,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "11",
        name: "Batt Style",
        setting_type: SettingType::Enum(&["Off", "Icon", "Percent", "Volts"]),
    },
    SettingMetadata {
        menu_num: "12",
        name: "Scan Range",
        setting_type: SettingType::Numeric {
            min: 1,
            max: 60000,
            unit: "MHz",
        },
    },
    SettingMetadata {
        menu_num: "13",
        name: "Scan Persist",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 200,
            unit: "s",
        },
    },
    SettingMetadata {
        menu_num: "14",
        name: "Scan Resume",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 250,
            unit: "s",
        },
    },
    SettingMetadata {
        menu_num: "15",
        name: "Ultra Scan",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 20,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "16",
        name: "Tone Monitor",
        setting_type: SettingType::Enum(&["Off", "On", "Clone"]),
    },
    SettingMetadata {
        menu_num: "17",
        name: "LCD Brightness",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 35,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "18",
        name: "LCD Timeout",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 250,
            unit: "s",
        },
    },
    SettingMetadata {
        menu_num: "19",
        name: "Breathe",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 30,
            unit: "s",
        },
    },
    SettingMetadata {
        menu_num: "20",
        name: "DTMF Dev",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 127,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "21",
        name: "Gamma",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 3,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "22",
        name: "Repeater Tone",
        setting_type: SettingType::Numeric {
            min: 100,
            max: 4000,
            unit: "Hz",
        },
    },
    SettingMetadata {
        menu_num: "23",
        name: "VFO A Group",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 15,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "24",
        name: "VFO A LastGrp",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 15,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "25",
        name: "VFO A Mode",
        setting_type: SettingType::Enum(&["VFO", "Channel"]),
    },
    SettingMetadata {
        menu_num: "26",
        name: "VFO B Group",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 15,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "27",
        name: "VFO B LastGrp",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 15,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "28",
        name: "VFO B Mode",
        setting_type: SettingType::Enum(&["VFO", "Channel"]),
    },
    SettingMetadata {
        menu_num: "29",
        name: "Key Lock",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "30",
        name: "Bluetooth",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "31",
        name: "Power Save",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 20,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "32",
        name: "Key Tones",
        setting_type: SettingType::Enum(&["Off", "On", "Diff", "Voice"]),
    },
    SettingMetadata {
        menu_num: "33",
        name: "STE",
        setting_type: SettingType::Enum(&["Off", "RX", "TX", "Both"]),
    },
    SettingMetadata {
        menu_num: "34",
        name: "RF Gain",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 41,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "35",
        name: "S-Bar Style",
        setting_type: SettingType::Enum(&["Segment", "Stepped", "Solid"]),
    },
    SettingMetadata {
        menu_num: "36",
        name: "Sq Noise Lev",
        setting_type: SettingType::Numeric {
            min: 15,
            max: 55,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "37",
        name: "Last FMT Freq",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 1000000000,
            unit: "Hz",
        },
    },
    SettingMetadata {
        menu_num: "38",
        name: "VOX",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 15,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "39",
        name: "VOX Tail",
        setting_type: SettingType::Numeric {
            min: 5,
            max: 50,
            unit: "s",
        },
    },
    SettingMetadata {
        menu_num: "40",
        name: "TX Timeout",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 250,
            unit: "s",
        },
    },
    SettingMetadata {
        menu_num: "41",
        name: "Dimmer",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 14,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "42",
        name: "DTMF Speed",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 20,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "43",
        name: "Noise Gate",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "44",
        name: "Scan Update",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 50,
            unit: "s",
        },
    },
    SettingMetadata {
        menu_num: "45",
        name: "ASL Support",
        setting_type: SettingType::Enum(&["Off", "COS", "USB", "I-COS"]),
    },
    SettingMetadata {
        menu_num: "46",
        name: "Disable FMT",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "47",
        name: "PIN",
        setting_type: SettingType::Numeric {
            min: 1000,
            max: 9999,
            unit: "",
        },
    },
    SettingMetadata {
        menu_num: "48",
        name: "PIN Action",
        setting_type: SettingType::Enum(&["Off", "Unlock", "Power On"]),
    },
    SettingMetadata {
        menu_num: "49",
        name: "LCD Inverted",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "50",
        name: "AF Filters",
        setting_type: SettingType::Enum(&[
            "All",
            "Hi-Pass + Lo-Pass",
            "Hi-Pass + De-emf",
            "Hi-Pass Only",
            "Lo-Pass + De-emf",
            "Lo-Pass Only",
            "De-emphasis Only",
            "None",
            "FSK Mode",
        ]),
    },
    SettingMetadata {
        menu_num: "51",
        name: "IF Freq",
        setting_type: SettingType::Enum(&["8.46", "7.25", "6.35", "5.64", "5.08", "4.62", "4.23"]),
    },
    SettingMetadata {
        menu_num: "52",
        name: "SBar AlwaysOn",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "53",
        name: "Locked VFO",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "54",
        name: "VFO Lock Act",
        setting_type: SettingType::Boolean,
    },
    SettingMetadata {
        menu_num: "55",
        name: "Dual Watch Dly",
        setting_type: SettingType::Numeric {
            min: 1,
            max: 30,
            unit: "s",
        },
    },
    SettingMetadata {
        menu_num: "56",
        name: "SubTone Dev",
        setting_type: SettingType::Numeric {
            min: 0,
            max: 127,
            unit: "",
        },
    },
];
