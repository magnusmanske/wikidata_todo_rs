//! Taxon ranks as Wikispecies writes them (Latin and English, with the
//! typos the pages have), and their items on Wikidata.

pub const SUBGENUS: &str = "Q3238261";

/// Rank names, lower case, and their items. Some ranks have different
/// items in botany and zoology; this table has the zoological ones, see [`rank_item`].
const RANKS: &[(&str, &str)] = &[
    ("cultivar", "Q4886"),
    ("species", "Q7432"),
    ("speciea", "Q7432"),
    ("apecies", "Q7432"),
    ("spacies", "Q7432"),
    ("speies", "Q7432"),
    ("spcies", "Q7432"),
    ("genus", "Q34740"),
    ("genu", "Q34740"),
    ("genera", "Q34740"),
    ("family", "Q35409"),
    ("familiae", "Q35409"),
    ("famila", "Q35409"),
    ("familia", "Q35409"),
    ("order", "Q36602"),
    ("ordo", "Q36602"),
    ("kingdom", "Q36732"),
    ("class", "Q37517"),
    ("classis", "Q37517"),
    ("phylum", "Q38348"),
    ("nothosubspecies", "Q59772023"),
    ("subspecies", "Q68947"),
    ("subspecis", "Q68947"),
    ("subpecies", "Q68947"),
    ("subspeccies", "Q68947"),
    ("suspecies", "Q68947"),
    ("subspcies", "Q68947"),
    ("subspesies", "Q68947"),
    ("domain", "Q146481"),
    ("tribe", "Q227936"),
    ("tribus", "Q227936"),
    ("form", "Q5469884"),
    ("formae", "Q279749"),
    ("formas", "Q279749"),
    ("division", "Q334460"),
    ("subvariety", "Q630771"),
    ("cryptic species complex", "Q765940"),
    ("variety", "Q767728"),
    ("varietas", "Q767728"),
    ("varietate", "Q767728"),
    ("varietates", "Q767728"),
    ("varietatess", "Q767728"),
    ("varieties", "Q767728"),
    ("subphylum", "Q1153785"),
    ("nothospecies", "Q1306176"),
    ("nothopecies", "Q1306176"),
    ("superspecies", "Q1783100"),
    ("infraclass", "Q2007442"),
    ("superfamily", "Q2136103"),
    ("superfamilia", "Q2136103"),
    ("suprafamilia", "Q2136103"),
    ("infraphylum", "Q2361851"),
    ("subfamily", "Q2455704"),
    ("subfamiliae", "Q2455704"),
    ("subfamilia", "Q2455704"),
    ("subkingdom", "Q2752679"),
    ("infraorder", "Q2889003"),
    ("infraordo", "Q2889003"),
    ("cohorte", "Q2981883"),
    ("series", "Q3025161"),
    ("infrakingdom", "Q3150876"),
    ("section", "Q10861426"),
    ("sectio", "Q3181348"),
    ("sectione", "Q3181348"),
    ("subgenus", "Q3238261"),
    ("branch", "Q3418438"),
    ("subdomain", "Q3491996"),
    ("subdivision", "Q3491997"),
    ("superclass", "Q3504061"),
    ("forma specialis", "Q3825509"),
    ("forma", "Q3825509"),
    ("subtribe", "Q3965313"),
    ("subtribus", "Q3965313"),
    ("superphylum", "Q3978005"),
    ("group", "Q4150646"),
    ("infracohort", "Q4226087"),
    ("infrafamily", "Q5481039"),
    ("subclass", "Q5867051"),
    ("subclassis", "Q5867051"),
    ("suborder", "Q5867959"),
    ("subordo", "Q5867959"),
    ("surbordo", "Q5867959"),
    ("superorder", "Q5868144"),
    ("superordo", "Q5868144"),
    ("subsection", "Q10861375"),
    ("subsectione", "Q5998839"),
    ("subsectio", "Q5998839"),
    ("nothogenus", "Q6045742"),
    ("magnorder", "Q6054237"),
    ("supercohort", "Q6054425"),
    ("infralegion", "Q6054535"),
    ("sublegion", "Q6054637"),
    ("superlegion", "Q6054795"),
    ("parvorder", "Q6311258"),
    ("parvordo", "Q6311258"),
    ("grandorder", "Q6462265"),
    ("legion", "Q7504331"),
    ("mirorder", "Q7506274"),
    ("subcohorte", "Q7509617"),
    ("species group", "Q7574964"),
    ("epifamily", "Q10296147"),
    ("subseries", "Q13198444"),
    ("subser", "Q13198444"),
    ("subform", "Q13202655"),
    ("supertribe", "Q14817220"),
    ("supertribus", "Q14817220"),
    ("superkingdom", "Q19858692"),
    ("subterclass", "Q21061204"),
    ("hyporder", "Q21074316"),
    ("clade", "Q713623"),
    ("cladus", "Q713623"),
];

/// Ranks whose botanical item differs from the zoological one in [`RANKS`].
const BOTANICAL: &[(&str, &str)] = &[("form", "Q279749"), ("section", "Q3181348"), ("subsection", "Q5998839")];

/// The item of a rank (any case); in the botanical sense for plants, fungi and algae.
pub fn rank_item(rank: &str, botanical: bool) -> Option<&'static str> {
    let rank = rank.trim().to_lowercase();
    let find = |table: &[(&str, &'static str)]| table.iter().find(|(name, _)| *name == rank).map(|(_, item)| *item);
    botanical.then(|| find(BOTANICAL)).flatten().or_else(|| find(RANKS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks() {
        assert_eq!(rank_item("Species", false), Some("Q7432"));
        assert_eq!(rank_item("ordo", true), Some("Q36602"));
        assert_eq!(rank_item("section", false), Some("Q10861426"));
        assert_eq!(rank_item("Section", true), Some("Q3181348"));
        assert_eq!(rank_item("classification system", false), None);
    }
}
