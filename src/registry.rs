//! Registry codec for the Login (play) packet, field 7 (doc §6).
//!
//! Standard NBT with an empty root name. Six registries are sent, matching the
//! vanilla 1.20.1 set (doc §6.1). Biome numeric ids MUST match
//! `crate::biomes::BIOMES` order: `id` is the entry's index in the `value`
//! list, which is built directly from `BIOMES` order.

use crate::biomes::BIOMES;
use crate::nbt;

/// Build the full registry codec (doc §6).
pub fn registry_codec() -> nbt::Value {
    nbt::Value::compound(vec![
        (
            "minecraft:dimension_type",
            registry("minecraft:dimension_type", dimension_types()),
        ),
        (
            "minecraft:worldgen/biome",
            registry("minecraft:worldgen/biome", biome_entries()),
        ),
        ("minecraft:chat_type", registry("minecraft:chat_type", chat_types())),
        (
            "minecraft:damage_type",
            registry("minecraft:damage_type", damage_types()),
        ),
        (
            "minecraft:trim_pattern",
            registry("minecraft:trim_pattern", trim_patterns()),
        ),
        (
            "minecraft:trim_material",
            registry("minecraft:trim_material", trim_materials()),
        ),
    ])
}

// ---------------------------------------------------------------------------
// NBT helpers
// ---------------------------------------------------------------------------

fn b(v: bool) -> nbt::Value {
    nbt::Value::Byte(v as i8)
}

fn i(v: i32) -> nbt::Value {
    nbt::Value::Int(v)
}

fn l(v: i64) -> nbt::Value {
    nbt::Value::Long(v)
}

fn f(v: f32) -> nbt::Value {
    nbt::Value::Float(v)
}

fn d(v: f64) -> nbt::Value {
    nbt::Value::Double(v)
}

fn s(v: &str) -> nbt::Value {
    nbt::Value::String(v.to_string())
}

fn chat_component(key: &str) -> nbt::Value {
    nbt::Value::compound(vec![("translate", s(key))])
}

/// Wrap entries as `{ type, value: [{ name, id, element }, ...] }` (doc §6).
/// `id` is the entry's 0-based index in the list.
fn registry(key: &str, entries: Vec<(String, nbt::Value)>) -> nbt::Value {
    let value = nbt::Value::List(
        entries
            .into_iter()
            .enumerate()
            .map(|(id, (name, element))| {
                nbt::Value::compound(vec![
                    ("name", nbt::Value::String(name)),
                    ("id", i(id as i32)),
                    ("element", element),
                ])
            })
            .collect(),
    );
    nbt::Value::compound(vec![("type", s(key)), ("value", value)])
}

// ---------------------------------------------------------------------------
// minecraft:dimension_type — 4 entries (doc §6.2)
// ---------------------------------------------------------------------------

fn uniform_light(min: i32, max: i32) -> nbt::Value {
    nbt::Value::compound(vec![
        ("type", s("minecraft:uniform")),
        (
            "value",
            nbt::Value::compound(vec![("min_inclusive", i(min)), ("max_inclusive", i(max))]),
        ),
    ])
}

fn overworld_dimension(has_ceiling: bool) -> nbt::Value {
    nbt::Value::compound(vec![
        ("piglin_safe", b(false)),
        ("natural", b(true)),
        ("ambient_light", f(0.0)),
        ("infiniburn", s("#minecraft:infiniburn_overworld")),
        ("respawn_anchor_works", b(false)),
        ("has_skylight", b(true)),
        ("bed_works", b(true)),
        ("effects", s("minecraft:overworld")),
        ("has_raids", b(true)),
        ("logical_height", i(384)),
        ("coordinate_scale", d(1.0)),
        ("monster_spawn_light_level", uniform_light(0, 7)),
        ("monster_spawn_block_light_limit", i(0)),
        ("min_y", i(-64)),
        ("height", i(384)),
        ("ultrawarm", b(false)),
        ("has_ceiling", b(has_ceiling)),
    ])
}

fn dimension_types() -> Vec<(String, nbt::Value)> {
    let nether = nbt::Value::compound(vec![
        ("piglin_safe", b(true)),
        ("natural", b(false)),
        ("ambient_light", f(0.1)),
        ("fixed_time", l(18000)),
        ("infiniburn", s("#minecraft:infiniburn_nether")),
        ("respawn_anchor_works", b(true)),
        ("has_skylight", b(false)),
        ("bed_works", b(false)),
        ("effects", s("minecraft:the_nether")),
        ("has_raids", b(false)),
        ("logical_height", i(128)),
        ("coordinate_scale", d(8.0)),
        ("monster_spawn_light_level", i(7)),
        ("monster_spawn_block_light_limit", i(15)),
        ("min_y", i(0)),
        ("height", i(256)),
        ("ultrawarm", b(true)),
        ("has_ceiling", b(true)),
    ]);
    let end = nbt::Value::compound(vec![
        ("piglin_safe", b(false)),
        ("natural", b(false)),
        ("ambient_light", f(0.0)),
        ("fixed_time", l(6000)),
        ("infiniburn", s("#minecraft:infiniburn_end")),
        ("respawn_anchor_works", b(false)),
        ("has_skylight", b(true)),
        ("bed_works", b(false)),
        ("effects", s("minecraft:the_end")),
        ("has_raids", b(true)),
        ("logical_height", i(256)),
        ("coordinate_scale", d(1.0)),
        ("monster_spawn_light_level", i(0)),
        ("monster_spawn_block_light_limit", i(0)),
        ("min_y", i(0)),
        ("height", i(256)),
        ("ultrawarm", b(false)),
        ("has_ceiling", b(false)),
    ]);
    vec![
        ("minecraft:overworld".to_string(), overworld_dimension(false)),
        (
            "minecraft:overworld_caves".to_string(),
            overworld_dimension(true),
        ),
        ("minecraft:the_nether".to_string(), nether),
        ("minecraft:the_end".to_string(), end),
    ]
}

// ---------------------------------------------------------------------------
// minecraft:worldgen/biome — 64 entries, ids = index in `crate::biomes::BIOMES`
// ---------------------------------------------------------------------------

fn biome_entries() -> Vec<(String, nbt::Value)> {
    BIOMES
        .iter()
        .map(|name| ((*name).to_string(), biome_element(name)))
        .collect()
}

fn biome_element(name: &str) -> nbt::Value {
    let (temperature, downfall, precipitation, grass_modifier) = biome_props(name);
    let mut effects = vec![
        ("sky_color", i(7_907_327)),
        ("fog_color", i(12_638_463)),
        ("water_color", i(4_159_204)),
        ("water_fog_color", i(329_011)),
        (
            "mood_sound",
            nbt::Value::compound(vec![
                ("tick_delay", i(6000)),
                ("offset", d(2.0)),
                ("block_search_extent", i(8)),
                ("sound", s("minecraft:ambient.cave")),
            ]),
        ),
    ];
    if let Some(modifier) = grass_modifier {
        effects.push(("grass_color_modifier", s(modifier)));
    }
    nbt::Value::compound(vec![
        ("has_precipitation", b(precipitation)),
        ("temperature", f(temperature)),
        ("downfall", f(downfall)),
        ("effects", nbt::Value::compound(effects)),
    ])
}

/// (temperature, downfall, has_precipitation, grass_color_modifier).
/// Climate values approximate vanilla 1.20.1; colors use the standard defaults
/// except for the two vanilla grass-color modifiers.
fn biome_props(name: &str) -> (f32, f32, bool, Option<&'static str>) {
    match name {
        // Dry biomes
        "minecraft:desert" | "minecraft:badlands" | "minecraft:eroded_badlands"
        | "minecraft:wooded_badlands" | "minecraft:savanna" | "minecraft:savanna_plateau"
        | "minecraft:windswept_savanna" => (2.0, 0.0, false, None),
        // Nether (no precipitation)
        "minecraft:nether_wastes" | "minecraft:warped_forest" | "minecraft:crimson_forest"
        | "minecraft:soul_sand_valley" | "minecraft:basalt_deltas" => (2.0, 0.0, false, None),
        // The End (no precipitation)
        "minecraft:the_end" | "minecraft:end_barrens" | "minecraft:end_highlands"
        | "minecraft:small_end_islands" | "minecraft:end_midlands" | "minecraft:the_void" => {
            (0.5, 0.5, false, None)
        }
        // Snowy / frozen
        "minecraft:snowy_plains" | "minecraft:ice_spikes" | "minecraft:frozen_river" => {
            (0.0, 0.5, true, None)
        }
        "minecraft:snowy_taiga" => (-0.5, 0.4, true, None),
        "minecraft:snowy_beach" => (0.05, 0.3, true, None),
        "minecraft:frozen_ocean" | "minecraft:deep_frozen_ocean" => (0.0, 0.5, true, None),
        "minecraft:frozen_peaks" | "minecraft:jagged_peaks" => (-0.7, 0.9, true, None),
        "minecraft:snowy_slopes" => (-0.3, 0.9, true, None),
        "minecraft:grove" => (-0.2, 0.8, true, None),
        // Taiga family
        "minecraft:taiga" | "minecraft:old_growth_spruce_taiga" => (0.25, 0.8, true, None),
        "minecraft:old_growth_pine_taiga" => (0.3, 0.8, true, None),
        "minecraft:birch_forest" | "minecraft:old_growth_birch_forest" => (0.6, 0.6, true, None),
        // Windswept / rocky
        "minecraft:windswept_hills" | "minecraft:windswept_gravelly_hills"
        | "minecraft:windswept_forest" | "minecraft:stony_shore" => (0.2, 0.3, true, None),
        // Jungle
        "minecraft:jungle" | "minecraft:bamboo_jungle" => (0.95, 0.9, true, None),
        "minecraft:sparse_jungle" => (0.95, 0.8, true, None),
        // Grass-colour modifiers
        "minecraft:swamp" | "minecraft:mangrove_swamp" => (0.8, 0.9, true, Some("swamp")),
        "minecraft:dark_forest" => (0.7, 0.8, true, Some("dark_forest")),
        // Oceans
        "minecraft:warm_ocean" | "minecraft:lukewarm_ocean" | "minecraft:deep_lukewarm_ocean"
        | "minecraft:ocean" | "minecraft:deep_ocean" | "minecraft:cold_ocean"
        | "minecraft:deep_cold_ocean" => (0.5, 0.5, true, None),
        // Misc
        "minecraft:stony_peaks" => (1.0, 0.3, true, None),
        "minecraft:mushroom_fields" => (0.9, 1.0, true, None),
        // Temperate default (plains, forest, meadow, rivers, beaches, caves, ...)
        _ => (0.8, 0.4, true, None),
    }
}

// ---------------------------------------------------------------------------
// minecraft:chat_type — 7 entries (doc §6.4)
// ---------------------------------------------------------------------------

fn conversation(key: &str, params: &[&str]) -> nbt::Value {
    nbt::Value::compound(vec![
        ("translation_key", s(key)),
        (
            "parameters",
            nbt::Value::List(params.iter().map(|p| s(p)).collect()),
        ),
    ])
}

fn chat_type_element(
    chat_key: &str,
    chat_params: &[&str],
    narration_key: &str,
    narration_params: &[&str],
) -> nbt::Value {
    nbt::Value::compound(vec![
        ("chat", conversation(chat_key, chat_params)),
        ("narration", conversation(narration_key, narration_params)),
    ])
}

// Parameter names are a closed enum in 1.20.1: "sender", "target", "content".
// Anything else (e.g. "team") fails the client's registry codec decode and
// kills the join — keep this list vanilla-exact.
fn chat_types() -> Vec<(String, nbt::Value)> {
    vec![
        (
            "minecraft:chat".to_string(),
            chat_type_element(
                "chat.type.text",
                &["sender", "content"],
                "chat.type.text.narrate",
                &["sender", "content"],
            ),
        ),
        (
            "minecraft:emote_command".to_string(),
            chat_type_element(
                "chat.type.emote",
                &["sender", "content"],
                "chat.type.emote",
                &["sender", "content"],
            ),
        ),
        (
            "minecraft:msg_command_incoming".to_string(),
            chat_type_element(
                "commands.message.display.incoming",
                &["sender", "content"],
                "chat.type.text.narrate",
                &["sender", "content"],
            ),
        ),
        (
            "minecraft:msg_command_outgoing".to_string(),
            chat_type_element(
                "commands.message.display.outgoing",
                &["target", "content"],
                "chat.type.text.narrate",
                &["sender", "content"],
            ),
        ),
        (
            "minecraft:say_command".to_string(),
            chat_type_element(
                "chat.type.announcement",
                &["sender", "content"],
                "chat.type.text.narrate",
                &["sender", "content"],
            ),
        ),
        (
            "minecraft:team_msg_command_incoming".to_string(),
            chat_type_element(
                "chat.type.team.text",
                &["target", "sender", "content"],
                "chat.type.text.narrate",
                &["sender", "content"],
            ),
        ),
        (
            "minecraft:team_msg_command_outgoing".to_string(),
            chat_type_element(
                "chat.type.team.text",
                &["target", "sender", "content"],
                "chat.type.text.narrate",
                &["sender", "content"],
            ),
        ),
    ]
}

// ---------------------------------------------------------------------------
// minecraft:damage_type — 44 entries (the vanilla 1.20.1 set; see doc §6.4)
// ---------------------------------------------------------------------------

fn damage_element(message_id: &str, scaling: &str, exhaustion: f32) -> nbt::Value {
    nbt::Value::compound(vec![
        ("message_id", s(message_id)),
        ("scaling", s(scaling)),
        ("exhaustion", f(exhaustion)),
    ])
}

fn damage_types() -> Vec<(String, nbt::Value)> {
    // Vanilla 1.20.1 names (1.20.1 = 1.19.4 set + outside_border/generic_kill).
    // Later additions (spit, campfire, wind_charge, mace_smash, ender_pearl,
    // spear, sulfur_cube_hot) are deliberately absent.
    const NAMES: &[&str] = &[
        "arrow",
        "bad_respawn_point",
        "cactus",
        "cramming",
        "dragon_breath",
        "drown",
        "dry_out",
        "explosion",
        "fall",
        "falling_anvil",
        "falling_block",
        "falling_stalactite",
        "fireball",
        "fireworks",
        "fly_into_wall",
        "freeze",
        "generic",
        "generic_kill",
        "hot_floor",
        "in_fire",
        "in_wall",
        "indirect_magic",
        "lava",
        "lightning_bolt",
        "magic",
        "mob_attack",
        "mob_attack_no_aggro",
        "mob_projectile",
        "on_fire",
        "out_of_world",
        "outside_border",
        "player_attack",
        "player_explosion",
        "sonic_boom",
        "stalagmite",
        "starve",
        "sting",
        "sweet_berry_bush",
        "thorns",
        "thrown",
        "trident",
        "unattributed_fireball",
        "wither",
        "wither_skull",
    ];
    NAMES
        .iter()
        .map(|name| {
            // Approximate vanilla scaling/exhaustion: attacker-type damage scales
            // with difficulty and causes exhaustion.
            let (scaling, exhaustion) = match *name {
                "arrow" | "fireball" | "falling_anvil" | "falling_block" | "falling_stalactite"
                | "indirect_magic" | "magic" | "mob_attack" | "mob_attack_no_aggro"
                | "mob_projectile" | "player_attack" | "sting" | "thorns" | "thrown"
                | "trident" | "unattributed_fireball" | "wither_skull" => {
                    ("when_caused_by_living_non_player", 0.1)
                }
                _ => ("never", 0.0),
            };
            (
                format!("minecraft:{name}"),
                damage_element(name, scaling, exhaustion),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// minecraft:trim_pattern — 16 entries (doc §6.4)
// ---------------------------------------------------------------------------

fn trim_patterns() -> Vec<(String, nbt::Value)> {
    const PATTERNS: &[&str] = &[
        "coast",
        "dune",
        "eye",
        "host",
        "raiser",
        "rib",
        "sentry",
        "shaper",
        "silence",
        "snout",
        "spire",
        "tide",
        "vex",
        "ward",
        "wayfinder",
        "wild",
    ];
    PATTERNS
        .iter()
        .map(|p| {
            (
                format!("minecraft:{p}"),
                nbt::Value::compound(vec![
                    ("asset_id", s(&format!("minecraft:{p}"))),
                    (
                        "template_item",
                        s(&format!("minecraft:{p}_armor_trim_smithing_template")),
                    ),
                    ("description", chat_component(&format!("trim_pattern.minecraft.{p}"))),
                ]),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// minecraft:trim_material — 10 entries (doc §6.4)
// ---------------------------------------------------------------------------

fn trim_materials() -> Vec<(String, nbt::Value)> {
    const MATERIALS: &[(&str, &str)] = &[
        ("amethyst", "minecraft:amethyst_shard"),
        ("copper", "minecraft:copper_ingot"),
        ("diamond", "minecraft:diamond"),
        ("emerald", "minecraft:emerald"),
        ("gold", "minecraft:gold_ingot"),
        ("iron", "minecraft:iron_ingot"),
        ("lapis_lazuli", "minecraft:lapis_lazuli"),
        ("netherite", "minecraft:netherite_ingot"),
        ("quartz", "minecraft:quartz"),
        ("redstone", "minecraft:redstone"),
    ];
    MATERIALS
        .iter()
        .enumerate()
        .map(|(idx, (name, ingredient))| {
            (
                format!("minecraft:{name}"),
                nbt::Value::compound(vec![
                    ("asset_name", s(name)),
                    ("ingredient", s(ingredient)),
                    ("item_model_index", f(idx as f32 * 0.1)),
                    ("description", chat_component(&format!("trim_material.minecraft.{name}"))),
                ]),
            )
        })
        .collect()
}
