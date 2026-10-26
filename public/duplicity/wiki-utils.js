const SPECIES_MOBILE_HOST = 'species.m.wikimedia.org';

const LANGUAGE_CODE_OVERRIDES = {
    als: 'gsw',
    bat_smg: 'sgs',
    be_x_old: 'be-tarask',
    bh: 'bho',
    commons: 'en',
    fiu_vro: 'vro',
    no: 'nb',
    roa_rup: 'rup',
    simple: 'en',
    species: 'en',
    zh_classical: 'lzh',
    zh_min_nan: 'nan',
    zh_yue: 'yue',
};

export function mobileServerForWiki(wiki) {
    if (wiki === 'specieswiki') return SPECIES_MOBILE_HOST;
    let m = wiki.match(/^(.+)wiki$/);
    if (m) return `${m[1]}.m.wikipedia.org`;
    m = wiki.match(/^(.+)(wik.+)$/);
    if (m) return `${m[1]}.m.${m[2]}.org`;
    return undefined;
}

export function desktopServerForWiki(wiki) {
    return mobileServerForWiki(wiki)?.replace('.m.', '.');
}

export function languageCodeForWiki(wiki) {
    const code = wiki.replace(/wiki.*$/, '');
    return LANGUAGE_CODE_OVERRIDES[code] ?? code;
}
