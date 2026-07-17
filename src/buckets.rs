/// Semantic hand-bucket classification for the GUI.
///
/// Returns small integer codes (0..=17) so the frontend can map them to
/// human-readable labels without paying for string payload per combo.
///
/// Bucket codes are ordered by approximate made-hand strength.

use crate::card::{rank, Hand};
use crate::eval::{evaluate, HandRank, Strength};

pub const STRAIGHT_FLUSH: u8 = 0;
pub const QUADS: u8 = 1;
pub const FULL_HOUSE: u8 = 2;
pub const FLUSH: u8 = 3;
pub const STRAIGHT: u8 = 4;
pub const SET: u8 = 5;
pub const TRIPS: u8 = 6;
pub const TWO_PAIR: u8 = 7;
pub const OVERPAIR: u8 = 8;
pub const TOP_PAIR: u8 = 9;
pub const SECOND_PAIR: u8 = 10;
pub const THIRD_PAIR: u8 = 11;
pub const FOURTH_PAIR: u8 = 12;
pub const FIFTH_PAIR: u8 = 13;
pub const UNDERPAIR: u8 = 14;
pub const ACE_HIGH: u8 = 15;
pub const KING_HIGH: u8 = 16;
pub const QUEEN_HIGH_OR_WORSE: u8 = 17;

/// Human-readable label for a bucket code.
pub fn bucket_label(code: u8) -> &'static str {
    match code {
        STRAIGHT_FLUSH => "Straight Flush",
        QUADS => "Quads",
        FULL_HOUSE => "Full House",
        FLUSH => "Flush",
        STRAIGHT => "Straight",
        SET => "Set",
        TRIPS => "Trips",
        TWO_PAIR => "Two Pair",
        OVERPAIR => "Overpair",
        TOP_PAIR => "Top Pair",
        SECOND_PAIR => "2nd Pair",
        THIRD_PAIR => "3rd Pair",
        FOURTH_PAIR => "4th Pair",
        FIFTH_PAIR => "5th Pair",
        UNDERPAIR => "Underpair",
        ACE_HIGH => "Ace High",
        KING_HIGH => "King High",
        QUEEN_HIGH_OR_WORSE => "Queen High or worse",
        _ => "Unknown",
    }
}

/// Classify a hole hand + board into a bucket code.
pub fn hand_bucket(hole: Hand, board: Hand) -> u8 {
    let all = hole.union(board);
    let strength = evaluate(all);
    match strength.hand_rank() {
        HandRank::StraightFlush => STRAIGHT_FLUSH,
        HandRank::FourOfAKind => QUADS,
        HandRank::FullHouse => FULL_HOUSE,
        HandRank::Flush => FLUSH,
        HandRank::Straight => STRAIGHT,
        HandRank::ThreeOfAKind => classify_three_of_a_kind(strength, hole),
        HandRank::TwoPair => TWO_PAIR,
        HandRank::OnePair => classify_one_pair(strength, hole, board),
        HandRank::HighCard => classify_high_card(strength),
    }
}

fn classify_three_of_a_kind(strength: Strength, hole: Hand) -> u8 {
    let trip_rank = first_kicker(strength) as u8;
    let hole_ranks = hole.iter().map(rank);
    let matches = hole_ranks.filter(|&r| r == trip_rank).count();
    if matches >= 2 {
        SET
    } else {
        TRIPS
    }
}

fn classify_one_pair(strength: Strength, hole: Hand, board: Hand) -> u8 {
    let pair_rank = first_kicker(strength) as u8;
    let mut hole_ranks = hole.iter().map(rank);
    let is_pocket_pair = hole_ranks.all(|r| r == pair_rank);

    if is_pocket_pair {
        let top_board_rank = distinct_board_ranks_desc(board).first().copied();
        if top_board_rank.map_or(true, |top| pair_rank > top) {
            OVERPAIR
        } else {
            UNDERPAIR
        }
    } else {
        pair_bucket(pair_rank, board)
    }
}

fn pair_bucket(pair_rank: u8, board: Hand) -> u8 {
    for (i, &r) in distinct_board_ranks_desc(board).iter().enumerate() {
        if r == pair_rank {
            return match i {
                0 => TOP_PAIR,
                1 => SECOND_PAIR,
                2 => THIRD_PAIR,
                3 => FOURTH_PAIR,
                4 => FIFTH_PAIR,
                _ => UNDERPAIR,
            };
        }
    }
    // Should not happen for a board-made pair; treat as underpair.
    UNDERPAIR
}

fn classify_high_card(strength: Strength) -> u8 {
    match first_kicker(strength) {
        12 => ACE_HIGH,          // Ace
        11 => KING_HIGH,
        _ => QUEEN_HIGH_OR_WORSE, // 10 = Queen and below
    }
}

fn first_kicker(strength: Strength) -> u8 {
    ((strength.0 >> 16) & 0xF) as u8
}

fn distinct_board_ranks_desc(board: Hand) -> Vec<u8> {
    let mut counts = [0u8; 13];
    for c in board.iter() {
        counts[rank(c) as usize] += 1;
    }
    let mut ranks = Vec::new();
    for r in (0..13u8).rev() {
        if counts[r as usize] > 0 {
            ranks.push(r);
        }
    }
    ranks
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{parse_cards, Hand};

    fn h(cards: &str) -> Hand {
        let mut hand = Hand::new();
        for c in parse_cards(cards).unwrap_or_default() {
            hand = hand.add(c);
        }
        hand
    }

    #[test]
    fn test_top_second_third_pair() {
        let board = h("AhKd7c");
        assert_eq!(hand_bucket(h("AdQh"), board), TOP_PAIR);
        assert_eq!(hand_bucket(h("KhJd"), board), SECOND_PAIR);
        assert_eq!(hand_bucket(h("7h6d"), board), THIRD_PAIR);
    }

    #[test]
    fn test_overpair_and_underpair() {
        let board = h("KhQd7c");
        assert_eq!(hand_bucket(h("AdAh"), board), OVERPAIR);
        assert_eq!(hand_bucket(h("8h8d"), board), UNDERPAIR);
    }

    #[test]
    fn test_high_card() {
        let board = h("AhKd7c");
        assert_eq!(hand_bucket(h("QhJd"), board), ACE_HIGH);
        assert_eq!(hand_bucket(h("JhTd"), board), ACE_HIGH);
        assert_eq!(hand_bucket(h("9h8d"), board), ACE_HIGH);
    }

    #[test]
    fn test_set_and_trips() {
        let board = h("AhKd7c");
        assert_eq!(hand_bucket(h("AdAc"), board), SET);
        assert_eq!(hand_bucket(h("KhKs"), board), SET);

        let paired_board = h("7h7dAc");
        assert_eq!(hand_bucket(h("7cKd"), paired_board), TRIPS);
    }

    #[test]
    fn test_two_pair_and_full_house() {
        let board = h("AsKd7c");
        assert_eq!(hand_bucket(h("AdKh"), board), TWO_PAIR);

        let trip_board = h("7h7d7c");
        assert_eq!(hand_bucket(h("AhAd"), trip_board), FULL_HOUSE);
    }

    #[test]
    fn test_quads() {
        let trip_board = h("7h7d7c");
        assert_eq!(hand_bucket(h("7s6d"), trip_board), QUADS);
    }

    #[test]
    fn test_flush_and_straight() {
        let flush_board = h("2h5h9h");
        assert_eq!(hand_bucket(h("JhTh"), flush_board), FLUSH);

        let straight_board = h("9hTdJs");
        assert_eq!(hand_bucket(h("QhKd"), straight_board), STRAIGHT);
    }

    #[test]
    fn test_fourth_and_fifth_pair() {
        let turn_board = h("AhKd9c7h");
        assert_eq!(hand_bucket(h("9d8c"), turn_board), THIRD_PAIR);
        assert_eq!(hand_bucket(h("7s6d"), turn_board), FOURTH_PAIR);

        let river_board = h("AhKd9c7h5s");
        assert_eq!(hand_bucket(h("7d6c"), river_board), FOURTH_PAIR);
        assert_eq!(hand_bucket(h("5d4c"), river_board), FIFTH_PAIR);
    }

    #[test]
    fn test_board_pair_one_pair_classification() {
        // Board has a pair of 7s; hand KQ makes the board pair (7s).
        // Distinct ranks desc: [A, 7]; 7 is at index 1 -> 2nd Pair.
        let board = h("7h7dAc");
        assert_eq!(hand_bucket(h("KhQd"), board), SECOND_PAIR);
    }
}
