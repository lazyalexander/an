use crate::plan::AskReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Selection {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

pub fn identify(masks: &[Vec<(u32, u32)>], selection: Selection) -> Result<usize, AskReason> {
    let mut counts: Vec<(usize, usize)> = masks
        .iter()
        .enumerate()
        .map(|(index, pixels)| {
            let count = pixels
                .iter()
                .filter(|pixel| inside(selection, **pixel))
                .count();
            (index, count)
        })
        .collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let winner = counts.first().copied().unwrap_or((0, 0));
    let runner = counts.get(1).map(|item| item.1).unwrap_or(0);
    if winner.1 < 8 || winner.1 * 2 < runner * 3 {
        return Err(AskReason::UnknownObject);
    }
    Ok(winner.0)
}

fn inside(selection: Selection, (x, y): (u32, u32)) -> bool {
    x >= selection.x
        && y >= selection.y
        && x < selection.x.saturating_add(selection.w)
        && y < selection.y.saturating_add(selection.h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(y: u32, x0: u32, n: u32) -> Vec<(u32, u32)> {
        (x0..x0 + n).map(|x| (x, y)).collect()
    }

    #[test]
    fn picks_the_mask_with_the_larger_overlap() {
        let masks = vec![row(0, 0, 40), row(1, 0, 10)];
        let selection = Selection {
            x: 0,
            y: 0,
            w: 40,
            h: 2,
        };
        assert_eq!(identify(&masks, selection), Ok(0));
    }

    #[test]
    fn refuses_a_close_call_or_a_tiny_overlap() {
        let tied = vec![row(0, 0, 10), row(1, 0, 10)];
        let selection = Selection {
            x: 0,
            y: 0,
            w: 10,
            h: 2,
        };
        assert_eq!(identify(&tied, selection), Err(AskReason::UnknownObject));
        let small = vec![row(0, 0, 7)];
        assert_eq!(
            identify(
                &small,
                Selection {
                    x: 0,
                    y: 0,
                    w: 7,
                    h: 1
                }
            ),
            Err(AskReason::UnknownObject)
        );
    }

    #[test]
    fn refuses_a_selection_that_misses_every_mask() {
        let masks = vec![row(0, 0, 20)];
        assert_eq!(
            identify(
                &masks,
                Selection {
                    x: 30,
                    y: 0,
                    w: 4,
                    h: 1
                }
            ),
            Err(AskReason::UnknownObject)
        );
    }
}
