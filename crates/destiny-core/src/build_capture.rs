//! DIM build capture rules, independent of acquisition and storage.
use crate::{ItemDefinition, ItemInstance, ObjectiveDefinition};

/// DIM crafted.ts:65-90 requires level, level-progress and timestamp objectives.
/// Timestamp is provider seconds, not an acquisition date or instance ID.
pub fn crafted_date_from_objectives(
    objectives: &[(u32, Option<u32>)],
    definitions: &[ObjectiveDefinition],
) -> Option<u64> {
    let (mut level, mut progress, mut date) = (None, false, None);
    for (hash, value) in objectives {
        match definitions
            .iter()
            .find(|d| d.hash() == *hash)
            .and_then(|d| d.ui_style())
        {
            Some(2) => level = *value,
            // DIM computes a ratio here; even NaN is not undefined. Only the
            // presence of this objective matters for capturing the date.
            Some(3) => progress = true,
            Some(4) => date = value.map(u64::from),
            _ => {}
        }
    }
    (level.is_some() && progress).then_some(date).flatten()
}

/// DIM known-values.ts:26-31, specialty-modslots.ts:49-125, mod-utils.ts:53-65.
pub fn insertable_armor_mod(definition: &ItemDefinition) -> bool {
    definition.armor.plug_category_hash.is_some_and(|hash| {
        [
            3422420680, 1526202480, 912441879, 2487827355, 2912171003, 2111701510, 13646368,
            125494331, 1486918022, 1703496685, 2106680364, 2140235634, 2173937871, 2207493141,
            2274750776, 1081029832, 3773173029, 3481777685,
        ]
        .contains(&hash)
    }) && definition
        .bucket_hash
        .is_none_or(|b| b.value() != 3313201758)
        && definition.item_type.is_some()
        && !empty_plug(definition.hash.value())
}

/// Pinned DIM data/d2/empty-plug-hashes.ts. Names are not identity evidence.
pub fn empty_plug(hash: i64) -> bool {
    [
        235531041, 3545404847, 1892982956, 1390587439, 1608119540, 702981643, 1959648454,
        2931483505, 3854296178, 2325217837, 4248210736, 2794014115, 1675508353, 791435474,
        3074755706, 2802541735, 518663192, 1715180370, 3875863236, 4037640975, 2801436041,
        2789698445, 3635963100, 321296654, 3416473448, 3207138885, 662916127, 3472368310,
        1080004479, 3819991001, 2352766955, 2164407902, 3834374608, 1007199041, 1527687869,
        2836298415, 3471922734, 1498917124, 1649663920, 1961918267, 253922071, 1826298670,
        3363787531, 3251563851, 2808665197, 3720092164, 424005861, 4205702044, 330751742,
        1618645595, 2111549310, 770211541, 1372656116, 1372656117, 1219897208, 366474809,
        1779961758, 1232390730, 3057124503, 3803329707, 2909846572, 666522389, 481675395,
        4173924323, 4055462131, 706611068, 3738398030, 2447143568, 1728096240, 4144354978,
        717667840, 4059283783, 720857, 1180997867, 2620967748, 4106547009, 1679876242, 3625698764,
        1182150429, 1835369552, 2600899007, 3851138800, 1285086138, 1844045567, 3820147479,
        1659393211, 1803434835, 3965359154, 1137289077, 3200810407, 4059708161, 1980618587,
        787139317, 807186981, 1078080765, 79843948, 573150099, 2269836811, 144338558, 2323986101,
        350414343, 2287797791, 3224071925, 2542427991, 51925409, 1134447515, 469511105, 2503665585,
        819232495, 2121121504, 4043342755, 4216349042, 2085536058, 1656746282, 2426387438,
        3725942064, 1692473496, 760030801,
    ]
    .contains(&hash)
}

pub fn crafted_date(
    item: &ItemInstance,
    definition: &ItemDefinition,
    objectives: &[ObjectiveDefinition],
) -> Option<u64> {
    if !definition
        .bucket_hash
        .is_some_and(|b| matches!(b.value(), 1498876634 | 2465295065 | 953998645))
    {
        return None;
    }
    let index = *definition
        .armor
        .socket_categories
        .iter()
        .find(|c| c.socket_category_hash == 3583996951)?
        .socket_indexes
        .first()?;
    let socket = item
        .sockets
        .iter()
        .find(|s| u32::from(s.socket_index) == index)?;
    crafted_date_from_objectives(socket.plug_objectives.as_deref()?, objectives)
}
