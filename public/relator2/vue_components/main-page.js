import { state, ttMixin } from '../../resources/vue_es6/state.js';
import { WikiDataItem } from '../../resources/js_es6/wikidata.js';
import {
	addItem,
	appState,
	bus,
	clearItemRelations,
	getWd,
	resetItems,
	setMainQ,
	setSourceUrl,
} from '../app-state.js';
import {
	isAncestorOf,
	isBiologicalAncestorOf,
	isFemale,
	isMale,
	isRelativeOf,
	isSiblingOf,
	isSpouseOf,
	getGenderedMirrorProperty,
	hasUnknownParent,
	knownParents,
	labelForProperty,
	liveRelationItems,
	maybe_default_properties,
	mirror_properties,
	mutualSiblingsInPool,
	named_items,
	named_properties,
	recomputeItemRelations,
	relation_properties,
	connectedItems,
} from '../relations.js';
import {
	isNonEmptyString,
	isValidURL,
	nameHasSurname,
	normalizeDateInput,
	numericSortItems,
	parseDate,
	relationTargetsToLoad,
	startTimeQualifier,
	surnameFromLabel,
	toWikidataDate,
	wikidataTimeValue,
} from '../utils.js';
import { apiJson } from '../api-client.js';
import { queueForItem } from '../write-queue.js';
import { ensureLoggedIn, isAuthError, isLoggedOut, NOT_LOGGED_IN_MESSAGE } from '../../resources/vue_es6/widar.js';
import { isRejected } from '../rejection_store.js';
import { relationWarnings, itemRelationWarnings, isImpossibleRelation, timeValueToInterval } from '../sanity.js';
import './people.js';
import './graph-view.js';

const TOOL_HASHTAG = 'Relator2';
const EDIT_SUMMARY = `Adding statement(s) using [[:toollabs:wikidata-todo/relator2|${TOOL_HASHTAG}]]`;
const NEW_ITEM_SUMMARY = `New item using [[:toollabs:wikidata-todo/relator2|${TOOL_HASHTAG}]]`;
const DESCRIPTION_SUMMARY = `Editing description using [[:toollabs:wikidata-todo/relator2|${TOOL_HASHTAG}]]`;

const WIKIMEDIA_URL = /^https?:\/\/[^\/]*?\.(wikipedia|wikimedia|commons|wikidata|wikisource|wikibooks)\.org\//;
const REFERENCE_URL_PROP = 'P854';
const REFERENCE_WIKIMEDIA_PROP = 'P4656';
const REFERENCE_RETRIEVED_PROP = 'P813';
const HUMAN = 'Q5';
const INSTANCE_OF = 'P31';
const BIRTH_PROP = 'P569';
const DEATH_PROP = 'P570';
const START_TIME_PROP = 'P580';

const WIKIDATA_API = 'https://www.wikidata.org/w/api.php';
const LOCAL_API = './api.php';
const NAME_GENDER_API_BASE = 'https://wd-infernal.toolforge.org/name_gender/';

const ITEM_ID_RE = /^Q[1-9]\d*$/;
// Stand-in QID for the not-yet-created person while validating its relation to
// the main item; never sent to Wikidata.
const PENDING_NEW_Q = '__new__';
const HELP_URL = 'https://www.wikidata.org/wiki/Wikidata:Relator2';
// Starts the OAuth dance; Widar sends the user back to this tool afterwards.
const LOGIN_URL = './api.php?action=authorize';
const NOTICE_STORAGE_KEY = 'relator2_features_notice_dismissed';

function isNoticeDismissed() {
	try { return localStorage.getItem(NOTICE_STORAGE_KEY) === '1'; }
	catch { return false; }
}
function rememberNoticeDismissed() {
	try { localStorage.setItem(NOTICE_STORAGE_KEY, '1'); } catch { /* storage unavailable */ }
}

// User-selectable people layouts. 'v2' and 'columns' consume the same
// generation-banded render_rows (see render()); they differ only in how those
// bands are presented (stacked horizontal rows vs. side-by-side scrolling
// columns). 'graph' is the interactive diagram (<graph-view>), which computes
// its own positions from appState and ignores render_rows. 'classic' is the
// older fixed three-band view, kept for reference.
const LAYOUT_STORAGE_KEY = 'relator2_layout';
// Selectable layouts. render() gates on this list; building render_rows for the
// graph layout is harmless (it simply goes unused). 'classic' is intentionally
// excluded (it predates render_rows).
const LAYOUTS = ['v2', 'columns', 'graph'];
const DEFAULT_LAYOUT = 'v2';

function loadLayout() {
	try {
		const v = localStorage.getItem(LAYOUT_STORAGE_KEY);
		return LAYOUTS.includes(v) ? v : DEFAULT_LAYOUT;
	} catch { return DEFAULT_LAYOUT; }
}
function storeLayout(v) {
	try { localStorage.setItem(LAYOUT_STORAGE_KEY, v); } catch { /* storage unavailable */ }
}

async function searchHumansMatching(name) {
	const d = await apiJson(WIKIDATA_API, {
		format: 'json', srnamespace: 0, srlimit: 500,
		action: 'query', list: 'search',
		srsearch: `${name} haswbstatement:${INSTANCE_OF}=${HUMAN}`,
	});
	return d.query.search.map((row) => row.title);
}

async function searchRelativesOf(q) {
	const haswbstatement = relation_properties.map((prop) => `${prop}=${q}`).join('|');
	const d = await apiJson(WIKIDATA_API, {
		format: 'json', srnamespace: 0, srlimit: 500, srprop: '',
		action: 'query', list: 'search',
		srsearch: `haswbstatement:${haswbstatement}`,
	});
	return d.query.search.map((row) => row.title);
}

async function fetchLiveEntity(q) {
	const d = await apiJson(WIKIDATA_API, {
		action: 'wbgetentities', ids: q, format: 'json',
	});
	const entities = d.entities ?? {};
	return entities[q] ?? Object.values(entities).find((e) => e?.id);
}

function liveHasClaim(entity, prop, target_q) {
	const claims = entity?.claims?.[prop] ?? [];
	return claims.some((c) => c.mainsnak?.datavalue?.value?.id === target_q);
}

// The Widar calls are callback-based. Promisifying them lets several actions
// wait for each other without blocking anything else.
function widarRequest(start) {
	return new Promise((resolve, reject) => {
		try {
			start(resolve);
		} catch (e) {
			reject(e);
		}
	});
}

const MainPage = Vue.extend({
	name: 'main-page',
	mixins: [ttMixin],
	props: ['initial_q'],
	data() {
		return {
			loading: false,
			new_q: '',
			invalid_q: '',
			suggestions: [],
			is_busy: {},
			add_item: { relation: '', name: '', gender: '', born: '', died: '', married: '' },
			// Writes that add claims to existing items; the shared suggestion
			// pool makes overlapping batches unsafe, so "Add checked" waits for
			// these while description saves and item creations run alongside.
			claims_in_flight: 0,
			// Item creations in flight. Each job carries its own draft, the main
			// item it belongs to, and its duplicate-check outcome, so the form can
			// be reused immediately for the next person.
			creation_jobs: [],
			creation_seq: 0,
			layout: loadLayout(),
			render_rows: [],
			pending_confirm: null,
			errors: [],
			notice_dismissed: isNoticeDismissed(),
			help_url: HELP_URL,
			unmounted: false,
		};
	},
	created() {
		setSourceUrl(this.$route.query.ref ?? '');
		bus.$on('error', this.onBusError);
		// A long-running job may finish after a route change has replaced this
		// component; the live instance rebuilds its view from appState then.
		bus.$on('refresh-view', this.refreshView);
		document.addEventListener('keydown', this.onLayoutHotkey);
		this.init();
	},
	beforeDestroy() {
		this.unmounted = true;
		bus.$off('error', this.onBusError);
		bus.$off('refresh-view', this.refreshView);
		document.removeEventListener('keydown', this.onLayoutHotkey);
	},
	watch: {
		$route() { this.init(); },
	},
	computed: {
		wd() { return getWd(); },
		main_q() { return appState.main_q; },
		items() { return appState.items; },
		source_url: {
			get() { return appState.source_url; },
			set(v) { setSourceUrl(v); },
		},
		named_items() { return named_items; },
		relation_properties() { return relation_properties; },
		is_spouse_relation() { return this.add_item.relation === named_properties.spouse; },
		// Reads the one shared login state, so the banner, the navbar and the
		// edit paths cannot disagree - and it survives this component being
		// recreated on every route change.
		login_required() { return isLoggedOut(); },
		login_url() { return LOGIN_URL; },
	},
	methods: {
		async loadRandom() {
			this.loading = true;
			try {
				const json = await apiJson(LOCAL_API, { action: 'random_person' });
				if (json.status !== 'OK' || !json.data) {
					this.loading = false;
					this.addError(json.status || 'No random person available');
					this.$router.replace('/');
					return;
				}
				this.$router.replace('/' + json.data);
			} catch (err) {
				this.loading = false;
				this.addError(`Random person lookup failed: ${err.message || err}`);
				this.$router.replace('/');
			}
		},

		init() {
			this.invalid_q = '';
			const raw = (this.initial_q ?? '').trim();
			if (raw === 'random') { this.loadRandom(); return; }
			if (isNonEmptyString(raw) && !ITEM_ID_RE.test(raw)) {
				this.invalid_q = raw;
				setMainQ('');
				this.resetAddItem();
				return;
			}
			setMainQ(raw);
			this.resetAddItem();
			if (!isNonEmptyString(appState.main_q)) return;
			this.loading = true;
			this.loadRelatives();
		},

		resetAddItem() {
			this.add_item = { relation: '', name: '', gender: '', born: '', died: '', married: '' };
		},
		// A draft that the user has not typed into can be replaced by a restored
		// failed creation without losing typed input.
		isAddItemEmpty() {
			return Object.values(this.add_item).every((v) => !isNonEmptyString(v));
		},
		canCreateNewItem() {
			return isNonEmptyString(this.add_item.gender)
				&& isNonEmptyString(this.add_item.name)
				&& isNonEmptyString(this.add_item.relation);
		},
		isValidMainProp(prop) {
			if (prop === named_properties.father && isFemale(this.main_q)) return false;
			if (prop === named_properties.mother && isMale(this.main_q)) return false;
			return true;
		},

		async loadRelatives() {
			try {
				addItem(this.main_q);
				const titles = await searchRelativesOf(this.main_q);
				resetItems(titles);
				addItem(this.main_q);
				await this.loadDirectRelations(appState.items);
				numericSortItems(appState.items);
				const loaded = await getWd().getItemBatch(appState.items);
				recomputeItemRelations(loaded);
				const kept = connectedItems(appState.items, appState.item_relations, this.main_q);
				if (kept.length !== appState.items.length) resetItems(kept);
				this.initializeIsBusy();
				this.generateSuggestions();
				this.render();
			} catch (err) {
				this.addError(`Loading relatives failed: ${err.message || err}`);
			} finally {
				this.loading = false;
				// The person cards only enter the DOM once `loading` is false, so
				// bring the main item into view after that flush. This covers both
				// a fresh page load and clicking a ⌖ icon (which navigates to a new
				// main item and re-runs init → loadRelatives via the $route watch).
				this.scrollToMainItem();
			}
		},
		// Scroll the main item's card into view if it isn't already fully visible.
		// Runs on the next tick so the just-rendered cards are in the DOM, and
		// skips quietly when there is no main card (e.g. a load error).
		scrollToMainItem() {
			this.$nextTick(() => {
				const el = this.$el?.querySelector?.('.person-card-main');
				if (!el) return;
				const rect = el.getBoundingClientRect();
				const vh = window.innerHeight || document.documentElement.clientHeight;
				const vw = window.innerWidth || document.documentElement.clientWidth;
				const fully_visible = rect.top >= 0 && rect.left >= 0
					&& rect.bottom <= vh && rect.right <= vw;
				if (fully_visible) return;
				el.scrollIntoView({ behavior: 'smooth', block: 'center', inline: 'center' });
			});
		},
		async loadDirectRelations(items) {
			const wd = getWd();
			// The batches below await; do not append to a family the user has
			// navigated away from in the meantime.
			const main_q = appState.main_q;
			const loaded = await wd.getItemBatch(items);
			recomputeItemRelations(loaded);
			const to_load = [];
			for (const q of items) {
				const item = wd.getItem(q);
				if (!item) continue;
				for (const prop of relation_properties) {
					for (const target_q of liveRelationItems(item, prop)) {
						if (items.includes(target_q)) continue;
						if (!to_load.includes(target_q)) to_load.push(target_q);
					}
				}
			}
			const loaded2 = await wd.getItemBatch(to_load);
			recomputeItemRelations(loaded2);
			for (const target_q of to_load) {
				if (appState.main_q !== main_q) return;
				if (wd.getItem(target_q) !== undefined) addItem(target_q);
			}
		},

		changeMainQ(q) {
			resetItems([]);
			clearItemRelations();
			this.$router.replace('/' + q);
		},
		openQ() {
			const q = this.new_q.trim();
			if (!isNonEmptyString(q)) return;
			this.$router.push('/' + q);
		},

		setLayout(v) {
			if (!LAYOUTS.includes(v)) return;
			this.layout = v;
			storeLayout(v);
			this.render();
		},
		cycleLayout() {
			const i = LAYOUTS.indexOf(this.layout);
			this.setLayout(LAYOUTS[(i + 1) % LAYOUTS.length]);
		},
		// Press "L" (when not typing in a field) to cycle Tree → Columns →
		// Diagram. Annotated in the layout dropdown's tooltip.
		onLayoutHotkey(e) {
			if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
			if ((e.key || '').toLowerCase() !== 'l') return;
			const t = e.target;
			const tag = (t?.tagName || '').toLowerCase();
			if (tag === 'input' || tag === 'textarea' || tag === 'select' || t?.isContentEditable) return;
			e.preventDefault();
			this.cycleLayout();
		},
		render() {
			this.render_rows = [];
			if (!LAYOUTS.includes(this.layout)) return;
			const rows = [[this.main_q]];
			const rowIndex = new Map();
			rowIndex.set(this.main_q, 0);
			this.renderTree(this.main_q, rows, rowIndex);
			const leftovers = appState.items.filter((q) => !rowIndex.has(q));
			if (leftovers.length > 0) rows.push(leftovers);
			this.render_rows = rows;
		},
		renderTree(q, rows, rowIndex) {
			if (!rowIndex.has(q)) return;
			let row = rowIndex.get(q);
			let pos = rows[row].indexOf(q);
			const todo = [];
			const unplaced = () => appState.items.filter((q2) => !rowIndex.has(q2));
			const placeInRow = (candidates, offset) => {
				for (const q2 of candidates) {
					rows[row].splice(pos + offset, 0, q2);
					if (offset === 0) pos++;
					rowIndex.set(q2, row);
					todo.push(q2);
				}
			};

			placeInRow(unplaced().filter((q2) => isSpouseOf(q, q2)), 0);
			placeInRow(unplaced().filter((q2) => isSiblingOf(q, q2)), 1);

			const ancestors = unplaced().filter((q2) => isAncestorOf(q2, q));
			if (ancestors.length > 0) {
				if (row === 0) {
					rows.splice(0, 0, []);
					for (const [k, v] of rowIndex) rowIndex.set(k, v + 1);
					row++;
				}
				for (const q2 of ancestors) {
					rows[row - 1].push(q2);
					rowIndex.set(q2, row - 1);
					todo.push(q2);
				}
			}

			const descendants = unplaced().filter((q2) => isAncestorOf(q, q2));
			if (descendants.length > 0) {
				if (row + 1 === rows.length) rows.push([]);
				for (const q2 of descendants) {
					rows[row + 1].push(q2);
					rowIndex.set(q2, row + 1);
					todo.push(q2);
				}
			}

			for (const q2 of todo) this.renderTree(q2, rows, rowIndex);
		},

		initializeIsBusy() {
			for (const q of appState.items) {
				if (!Object.hasOwn(this.is_busy, q)) Vue.set(this.is_busy, q, 0);
			}
		},
		isBusy() {
			return Object.values(this.is_busy).some((n) => n > 0);
		},
		// Saves the user can see running: per-item writes plus item creations.
		// A pending duplicate decision is not a save.
		activeJobCount() {
			const busy = Object.values(this.is_busy).filter((n) => n > 0).length;
			const creating = this.creation_jobs.filter((j) => j.status === 'checking' || j.status === 'creating').length;
			return busy + creating;
		},
		// A job started by a component instance that a route change has since
		// destroyed still finishes its write; the live instance redraws from the
		// shared cache.
		refreshView() {
			recomputeItemRelations(appState.items);
			this.initializeIsBusy();
			this.render();
			if (!this.isBusy()) this.generateSuggestions();
		},
		updateBusy(q, diff) {
			const current = typeof this.is_busy[q] === 'number' ? this.is_busy[q] : 0;
			Vue.set(this.is_busy, q, current + diff);
		},

		addError(msg) {
			if (msg === undefined || msg === null || msg === '') return;
			const text = String(msg);
			// A job may outlive the component instance that started it (route
			// change); hand its error to the live instance via the bus.
			if (this.unmounted) { bus.$emit('error', text); return; }
			// A batch of edits tends to fail for one and the same reason; saying
			// it once is enough.
			if (this.errors.includes(text)) return;
			this.errors.push(text);
		},
		// Every edit path starts here: a confirmed login before anything is
		// written, so an expired login gives one clear message instead of a mix
		// of successful and "not logged in" edits. An inconclusive check (server
		// unreachable) does not block the edit — the server decides then.
		async confirmLogin() {
			if (await ensureLoggedIn() !== false) return true;
			this.addError(NOT_LOGGED_IN_MESSAGE);
			return false;
		},
		recheckLogin() { ensureLoggedIn(true); },
		// Failures caused by the login are reported without the server's raw
		// (HTML) message, and re-check the login so the UI catches up.
		reportEditError(context, msg) {
			if (isAuthError(msg)) {
				this.recheckLogin();
				this.addError(`${context}: ${NOT_LOGGED_IN_MESSAGE}`);
				return;
			}
			this.addError(`${context}: ${msg}`);
		},
		onBusError(msg) { this.addError(msg); },
		dismissError(idx) { this.errors.splice(idx, 1); },
		clearErrors() { this.errors = []; },
		dismissNotice() {
			this.notice_dismissed = true;
			rememberNoticeDismissed();
		},

		ancestors() { return appState.items.filter((q) => isAncestorOf(q, this.main_q)); },
		descendants() { return appState.items.filter((q) => isAncestorOf(this.main_q, q)); },
		spouses() { return appState.items.filter((q) => isSpouseOf(this.main_q, q)); },
		relatives() { return appState.items.filter((q) => isRelativeOf(this.main_q, q)); },

		generateSuggestions() {
			const fresh = appState.items
				.flatMap((q) => this.generateSuggestionForQ(q))
				.filter((s) => s.from !== s.to)
				.filter((s) => !hasUnknownParent(getWd().getItem(s.from), s.prop))
				// Heuristic suggestions (a father's spouse as the mother, a
				// sibling's parent, a spouse deduced from a common child) can be
				// contradicted outright by the dates. Don't offer those at all;
				// the user only ever sees relations that are physically possible.
				// Relations the user adds by hand are not filtered here — those
				// still go through the save-time warning dialog.
				.filter((s) => !isImpossibleRelation(s))
				.filter((s) => !isRejected(s));
			const transient = this.suggestions.filter((s) => s.transient);
			const next = [...transient];
			for (const o of fresh) if (!this.doesSuggestionExist(o, next)) next.push(o);
			this.suggestions = next;
		},
		generateSuggestionForQ(root_q) {
			const wd = getWd();
			const root_i = wd.getItem(root_q);
			if (!root_i) return [];

			const my_ancestors = appState.items.filter((q) => isAncestorOf(q, root_q));
			const direct_siblings = liveRelationItems(root_i, named_properties.sibling);
			// Siblings sharing a common sibling with root, deduced transitively
			// when no parents tie them together (issue #81).
			const transitive_siblings = mutualSiblingsInPool(root_q, appState.items);
			const wider_siblings = appState.items.filter((q1) => {
				if (q1 === root_q) return false;
				if (direct_siblings.includes(q1)) return true;
				const i = wd.getItem(q1);
				if (i?.hasClaimItemLink(named_properties.sibling, root_q)) return true;
				if (transitive_siblings.includes(q1)) return true;
				const ancestors = appState.items.filter((q2) => isAncestorOf(q2, q1));
				return my_ancestors.some((a) => ancestors.includes(a));
			});
			const new_siblings = wider_siblings
				.filter((q) => !direct_siblings.includes(q))
				.map((q) => ({ from: root_q, prop: named_properties.sibling, to: q }));
			const sibling_mirrors = direct_siblings
				.map((q) => wd.getItem(q))
				.filter((target) => target && !target.hasClaimItemLink(named_properties.sibling, root_q))
				.map((target) => ({ from: target.getID(), prop: named_properties.sibling, to: root_q }));

			const wider_spouses = appState.items.filter((q) => isSpouseOf(q, root_q));
			const direct_spouses = liveRelationItems(root_i, named_properties.spouse);
			let new_spouses = wider_spouses
				.filter((q) => !direct_spouses.includes(q))
				.map((q) => ({ from: root_q, prop: named_properties.spouse, to: q }));

			// Only biological descendants are offered as children (P40); a
			// step- or god-child must never be suggested as a child (issue #79).
			const wider_descendants = appState.items.filter((q) => isBiologicalAncestorOf(root_q, q));
			const direct_children = liveRelationItems(root_i, named_properties.child);
			const new_children = wider_descendants
				.filter((q) => !direct_children.includes(q))
				.map((q) => ({ from: root_q, prop: named_properties.child, to: q }));

			const fathers = liveRelationItems(root_i, named_properties.father);
			const mothers = liveRelationItems(root_i, named_properties.mother);
			const all_fathers_items = my_ancestors
				.map((q) => wd.getItem(q))
				.filter((i) => i && !i.hasClaimItemLink(named_properties.gender, named_items.female));
			const all_mothers_items = my_ancestors
				.map((q) => wd.getItem(q))
				.filter((i) => i && !i.hasClaimItemLink(named_properties.gender, named_items.male));

			let new_parents = [];
			if (fathers.length > 0 && mothers.length === 0) {
				new_parents = fathers
					.map((q) => wd.getItem(q))
					.filter(Boolean)
					.flatMap((i) => liveRelationItems(i, named_properties.spouse))
					.map((q) => ({ from: root_q, prop: named_properties.mother, to: q, maybe: true }));
			}
			if (fathers.length === 0 && mothers.length > 0) {
				new_parents = mothers
					.map((q) => wd.getItem(q))
					.filter(Boolean)
					.flatMap((i) => liveRelationItems(i, named_properties.spouse))
					.map((q) => ({ from: root_q, prop: named_properties.father, to: q, maybe: true }));
			}
			if (fathers.length === 0) {
				const more_fathers = wider_siblings
					.map((q) => wd.getItem(q))
					.filter(Boolean)
					.flatMap((i) => liveRelationItems(i, named_properties.father))
					.map((q) => ({ from: root_q, prop: named_properties.father, to: q, maybe: true }));
				const more_mothers = wider_siblings
					.map((q) => wd.getItem(q))
					.filter(Boolean)
					.flatMap((i) => liveRelationItems(i, named_properties.mother))
					.map((q) => ({ from: root_q, prop: named_properties.mother, to: q, maybe: true }));
				new_parents = [...new_parents, ...more_fathers, ...more_mothers];
			}

			// Never offer a biological parent (P22/P25) we already know to be a
			// step-/god-parent — or another kind of parent — of this person. The
			// explicit relation already states the truth; the spouse-of-mother
			// and sibling's-parent heuristics must not contradict it (#79).
			const known_parents = knownParents(root_i);
			new_parents = new_parents.filter((s) => !known_parents.includes(s.to));

			new_spouses = [
				...new_spouses,
				...this.suggestSpouseViaCommonChild(all_fathers_items, all_mothers_items),
				...this.suggestSpouseViaCommonChild(all_mothers_items, all_fathers_items),
			];

			const mirror = [];
			for (const [prop, mirror_props] of Object.entries(mirror_properties)) {
				for (const q of liveRelationItems(root_i, prop)) {
					const target = wd.getItem(q);
					if (!target) continue;
					const mp = getGenderedMirrorProperty(root_i, mirror_props);
					if (mp === undefined) continue;
					if (target.hasClaimItemLink(mp, root_q)) continue;
					// Don't mirror a "relative" (P1038) onto someone who already
					// has a closer, more specific link to this person (a step-/
					// god-/blood ancestor, descendant or spouse). isRelativeOf
					// encodes exactly that exclusion (#79).
					if (prop === named_properties.relative && !isRelativeOf(q, root_q)) continue;
					mirror.push({
						from: target.getID(),
						prop: mp,
						to: root_q,
						maybe: maybe_default_properties.has(prop),
					});
				}
			}

			const all = [...mirror, ...sibling_mirrors, ...new_siblings, ...new_spouses, ...new_children, ...new_parents];
			for (const o of all) {
				o.suggestion = true;
				if (!o.maybe) o.selected = true;
			}
			return all;
		},
		suggestSpouseViaCommonChild(all1, all2) {
			const unmarried = all1
				.filter((i) => !i.hasClaims(named_properties.spouse))
				.filter((i) => !i.hasClaims(named_properties.unmarried_partner))
				.map((i) => i.getID());
			return all2.flatMap((i) => unmarried.map((q2) => ({
				from: q2, prop: named_properties.spouse, to: i.getID(), maybe: true,
			})));
		},

		addSuggestions(events) {
			const sofar = [...this.suggestions];
			const additions = [];
			for (const o of events) {
				o.suggestion = true;
				if (this.doesSuggestionExist(o, sofar)) continue;
				sofar.push(o);
				additions.push(o);
			}
			if (additions.length === 0) return;
			this.suggestions = [...this.suggestions, ...additions];

			// Drop-rule suggestions are marked transient and persisted immediately.
			const byFrom = new Map();
			for (const o of additions) {
				if (!o.transient) continue;
				if (!byFrom.has(o.from)) byFrom.set(o.from, []);
				byFrom.get(o.from).push(o);
			}
			for (const [q, list] of byFrom) this.persistEditsFor(q, list);
		},
		selectSuggestion(ev) {
			for (const s of this.suggestions) {
				if (s.from === ev.from && s.prop === ev.prop && s.to === ev.to) s.selected = ev.selected;
			}
		},
		// Add a sex/gender (P21) statement to an item that has none. Guards the
		// QID and whitelists the gender value before persisting; persistEditsFor
		// re-checks the live entity, so a no-op if the claim already exists.
		setGender(ev) {
			const q = ev?.q;
			const gender_q = ev?.gender_q;
			if (!ITEM_ID_RE.test(q ?? '')) return;
			if (gender_q !== named_items.male && gender_q !== named_items.female) return;
			const item = getWd().getItem(q);
			if (!item || item.hasClaims(named_properties.gender)) return;
			this.persistEditsFor(q, [{ from: q, prop: named_properties.gender, to: gender_q }]);
		},
		// Add or edit an item's description in the user's language. Uses
		// wbsetdescription, whose response is a partial entity, so we update the
		// description in place (never updateItemRaw, which would drop the item's
		// claims) and bump the per-item reactive signal. The write is queued
		// behind any other write to this item, while other items stay editable.
		// The originating person card keeps its editor open until the bus result
		// tells it the outcome, so the typed text survives a failed save.
		async saveDescription(ev) {
			const q = ev?.q;
			const lang = ev?.lang;
			const value = (ev?.text ?? '').trim();
			if (!ITEM_ID_RE.test(q ?? '') || !isNonEmptyString(lang) || value === '') return;
			if (getWd().getItem(q) === undefined) return;

			let ok = false;
			// Lock this card at once, so a slow login check cannot let a second
			// action start on the same item in the meantime.
			this.updateBusy(q, 1);
			try {
				let logged_in = false;
				try {
					logged_in = await this.confirmLogin();
				} catch (e) {
					this.addError(`Could not check login: ${e.message || e}`);
				}
				if (logged_in) {
					ok = await queueForItem(q, async () => {
						const d = await widarRequest((cb) =>
							state.widar.setDescription(q, lang, value, DESCRIPTION_SUMMARY, TOOL_HASHTAG, cb));
						if (d?.error !== 'OK') {
							this.reportEditError(`Could not save description for ${q}`, d?.error ?? 'unknown error');
							return false;
						}
						const saved = d.res?.entity?.descriptions?.[lang]?.value ?? value;
						this.applyDescription(q, lang, saved);
						return true;
					});
				}
			} catch (e) {
				this.addError(`Could not save description on ${q}: ${e.message || e}`);
			} finally {
				this.updateBusy(q, -1);
				if (!this.isBusy()) this.generateSuggestions();
			}
			bus.$emit('description-save-result', { q, ok });
		},
		applyDescription(q, lang, value) {
			const item = getWd().getItem(q);
			if (!item) return;
			item.setDesc(lang, value);
			// recomputeItemRelations re-publishes appState.item_relations[q], the
			// reactive signal the person card watches, forcing the header to
			// re-read the (now updated) description.
			recomputeItemRelations([q]);
		},
		doesSuggestionExist(o, existing = []) {
			if (existing.some((x) => x.from === o.from && x.to === o.to && x.prop === o.prop)) return true;
			const i = getWd().getItem(o.from);
			if (i === undefined) return true;
			return i.hasClaimItemLink(o.prop, o.to);
		},
		showAddCheckedButton() {
			return this.suggestions.some((s) => s.selected);
		},

		updateItemRaw(q, json) {
			const wd = getWd();
			const item = new WikiDataItem(wd, JSON.parse(JSON.stringify(json)));
			Vue.set(wd.items, q, item);
			recomputeItemRelations([q]);
		},

		// Attach a "start time" (P580) qualifier to a statement when a valid
		// date string is given; a no-op (returns the statement unchanged) otherwise.
		addMarriageQualifier(statement, date) {
			const qualifier = startTimeQualifier(date);
			if (!qualifier) return statement;
			statement.qualifiers = qualifier;
			statement['qualifiers-order'] = [START_TIME_PROP];
			return statement;
		},

		// `source_url` is passed in by jobs that may run after the user has typed a
		// different reference into the form (or navigated away), so a save cites
		// the source it was started with.
		addReferenceToStatement(statement, source_url = appState.source_url) {
			const url = source_url.trim();
			if (!isValidURL(url)) return statement;
			const property = WIKIMEDIA_URL.test(url) ? REFERENCE_WIKIMEDIA_PROP : REFERENCE_URL_PROP;
			const now = toWikidataDate(new Date());
			const urlSnak = {
				snaktype: 'value', property,
				datavalue: { value: url, type: 'string' },
			};
			const retrievedSnak = {
				snaktype: 'value', property: REFERENCE_RETRIEVED_PROP,
				datavalue: {
					value: {
						time: now, timezone: 0, before: 0, after: 0, precision: 11,
						calendarmodel: 'http://www.wikidata.org/entity/Q1985727',
					}, type: 'time'
				},
			};
			const reference = {
				snaks: {
					[property]: [urlSnak],
					[REFERENCE_RETRIEVED_PROP]: [retrievedSnak],
				},
				'snaks-order': [property, REFERENCE_RETRIEVED_PROP],
			};
			statement.references = [...(statement.references ?? []), reference];
			return statement;
		},

		onAddChecked() {
			if (this.claims_in_flight > 0) return;
			const selected = this.suggestions.filter((s) => s.selected);
			this.confirmIfWarnings(this.warningMessages(selected), () => this.commitSelected(selected));
			return false;
		},
		commitSelected(selected) {
			const items2edit = [...new Set(selected.map((s) => s.from))];
			for (const q of items2edit) this.persistEditsFor(q, selected.filter((s) => s.from === q));
		},
		// Collect de-duplicated logical-impossibility warnings (issue #75) for a
		// batch of proposed relations. `dateOverrides`/`labels` let a not-yet-
		// created person be checked via its form values.
		warningMessages(relations, dateOverrides = {}, labels = {}) {
			const seen = new Set();
			const out = [];
			const push = (message) => {
				if (seen.has(message)) return;
				seen.add(message);
				out.push(message);
			};
			for (const rel of relations) {
				for (const w of relationWarnings(rel, getWd(), { dateOverrides, labels })) push(w.message);
			}
			// Item-level: would the save leave the same person reached through
			// more than one relation? Check each affected item's existing live
			// relations together with the ones being added, but only surface a
			// warning when one of the *added* targets is the duplicate.
			const byFrom = new Map();
			for (const rel of relations) {
				if (!byFrom.has(rel.from)) byFrom.set(rel.from, []);
				byFrom.get(rel.from).push(rel);
			}
			for (const [from, added] of byFrom) {
				const existing = appState.item_relations[from] ?? [];
				const addedTargets = new Set(added.map((r) => r.to));
				for (const w of itemRelationWarnings([...existing, ...added], getWd(), { labels })) {
					if (addedTargets.has(w.target)) push(w.message);
				}
			}
			return out;
		},
		// Gate a save behind an acknowledgement dialog when warnings exist;
		// otherwise proceed immediately.
		confirmIfWarnings(warnings, proceed) {
			if (warnings.length > 0) { this.pending_confirm = { warnings, proceed }; return; }
			proceed();
		},
		confirmPendingSave() {
			const pending = this.pending_confirm;
			this.pending_confirm = null;
			pending?.proceed?.();
		},
		cancelPendingSave() { this.pending_confirm = null; },
		markTransientsSaved(q_suggestions, newlyAdded) {
			const fresh = new Set(newlyAdded || []);
			for (const s of q_suggestions) {
				if (!s.transient) continue;
				Vue.set(s, 'saved', fresh.has(s) ? 'new' : 'existed');
				Vue.set(s, 'saved_msg', '');
			}
		},
		markTransientsError(q_suggestions, message) {
			for (const s of q_suggestions) {
				if (!s.transient) continue;
				Vue.set(s, 'saved', 'error');
				Vue.set(s, 'saved_msg', message || 'save failed');
			}
		},
		async persistEditsFor(q, q_suggestions, source_url = appState.source_url) {
			const wd = getWd();
			if (wd.getItem(q) === undefined) return;

			// Lock this item before any await, so a second user action cannot
			// slip in while the login check is still running.
			this.updateBusy(q, 1);
			this.claims_in_flight++;
			let released = false;
			const release = () => {
				if (released) return;
				released = true;
				this.updateBusy(q, -1);
				this.claims_in_flight = Math.max(0, this.claims_in_flight - 1);
				if (!this.isBusy()) this.generateSuggestions();
			};

			let logged_in = false;
			try {
				logged_in = await this.confirmLogin();
			} catch (e) {
				this.addError(`Could not check login: ${e.message || e}`);
			}
			if (!logged_in) {
				this.markTransientsError(q_suggestions, 'not logged in');
				release();
				return;
			}

			try {
				let to_load = [];
				try {
					// Queueing keeps two writes to the same item from both seeing the
					// old state and adding the same claim twice.
					const outcome = await queueForItem(q, () => this.writeClaimsFor(q, q_suggestions, source_url));
					to_load = outcome.to_load;
				} catch (e) {
					this.addError(`Could not save edit to ${q}: ${e.message || e}`);
					this.markTransientsError(q_suggestions, `Could not save edit: ${e.message || e}`);
				}

				// Loading newly related items is read-only. Skip it when the user has
				// moved on to another family; the write itself already happened.
				if (to_load.length > 0 && appState.items.includes(q)) {
					try {
						await this.loadDirectRelations(to_load);
						numericSortItems(appState.items);
						const nonBusyItems = appState.items.filter((iq) => !(this.is_busy[iq] > 0));
						await wd.getItemBatch(nonBusyItems);
						bus.$emit('refresh-view');
					} catch (e) {
						this.addError(`Failed to refresh after edit to ${q}: ${e.message || e}`);
					}
				}
			} finally {
				release();
			}
		},
		// One queued claim write: re-read the live item, add only the claims that
		// are still missing, and apply the response to the shared cache. Returns
		// the relation targets to load afterwards (none on a no-op or an error).
		async writeClaimsFor(q, q_suggestions, source_url) {
			let live;
			try {
				live = await fetchLiveEntity(q);
			} catch (e) {
				this.addError(`Could not verify live Wikidata state for ${q}: ${e.message || e}`);
				this.markTransientsError(q_suggestions, `Could not verify live Wikidata state: ${e.message || e}`);
				return { to_load: [] };
			}
			if (live) this.updateItemRaw(q, live);
			const claimsToAdd = q_suggestions.filter((s) => !liveHasClaim(live, s.prop, s.to));
			if (claimsToAdd.length === 0) {
				this.markTransientsSaved(q_suggestions, []);
				return { to_load: [] };
			}

			const data = {
				claims: claimsToAdd.map((s) => this.addReferenceToStatement(this.statementFor(s), source_url)),
			};
			const to_load = relationTargetsToLoad(q_suggestions, appState.items, relation_properties);

			let d;
			try {
				d = await widarRequest((cb) =>
					state.widar.runWBEditEntity(q, data, EDIT_SUMMARY, TOOL_HASHTAG, cb));
			} catch (e) {
				this.addError(`Could not start edit on ${q}: ${e.message || e}`);
				this.markTransientsError(q_suggestions, `Could not start edit: ${e.message || e}`);
				return { to_load: [] };
			}

			if (d?.error !== 'OK' || d?.status !== 'OK') {
				const msg = `${d?.error || ''} / ${d?.status || ''}`.trim();
				this.reportEditError(`Could not save edit to ${q}`, msg);
				this.markTransientsError(q_suggestions, msg);
				return { to_load: [] };
			}

			this.updateItemRaw(q, d.res.entity);
			this.markTransientsSaved(q_suggestions, claimsToAdd);
			return { to_load };
		},
		statementFor(s) {
			const statement = {
				mainsnak: {
					snaktype: 'value',
					property: s.prop,
					datavalue: { value: { id: s.to, 'entity-type': 'item' }, type: 'wikibase-entityid' },
				},
				type: 'statement',
				rank: 'normal',
			};
			return this.addMarriageQualifier(statement, s.married);
		},

		async parseNewPersonName() {
			this.cleanNewPersonName();
			await this.detectNewPersonGender();
		},
		// Normalise the typed name and pull any "(…)" date range out into the
		// Born/Died fields. Synchronous; gender detection is handled separately
		// (see detectNewPersonGender) so callers can adjust the name in between.
		cleanNewPersonName() {
			this.parseDateFields();

			const split = this.add_item.name.split('(');
			if (split.length === 2) {
				this.add_item.name = split[0].trim();
				const dates = split[1];
				const parts = dates.split(/[\-\–\–;†]/, 2);
				if (parts.length === 2) {
					this.add_item.born = parseDate(parts[0]);
					this.add_item.died = parseDate(dates.slice(parts[0].length + 1));
				} else if (dates.includes('†')) {
					this.add_item.died = parseDate(dates);
				} else {
					this.add_item.born = parseDate(dates);
				}
			}

			this.add_item.name = this.add_item.name.replace(/,? *\b(geb\.|geborene) */, ' ');

			this.add_item.name = this.add_item.name
				.replace(/\[WD\]/gi, '')
				.replace(/[;,.:!?\s]+$/, '')
				.replace(/\s+/g, ' ')
				.trim();
		},
		// Normalise the free-text date fields in place, so "Parse" shows the user
		// what will actually be written. A field already in Wikidata-ready form
		// is left alone (normalizeDateInput), and text nothing date-like can be
		// found in is kept as typed rather than blanked — the user can still fix
		// it, and createItemFromInterface reports it if they don't.
		parseDateFields() {
			for (const field of ['born', 'died', 'married']) {
				const parsed = normalizeDateInput(this.add_item[field]);
				if (isNonEmptyString(parsed)) this.add_item[field] = parsed;
			}
		},
		async detectNewPersonGender() {
			if (isNonEmptyString(this.add_item.gender)) return;

			if (this.add_item.relation === named_properties.spouse) {
				if (isMale(this.main_q)) this.add_item.gender = named_items.female;
				if (isFemale(this.main_q)) this.add_item.gender = named_items.male;
				if (isNonEmptyString(this.add_item.gender)) return;
			}

			await this.guessGenderFromName();
		},
		// The new person inherits the main item's family name only when they are
		// a child (main is their father/mother) or a sibling — relations where a
		// shared surname is the norm. Spouses, parents, etc. are excluded.
		showAddSurnameButton() {
			const inheriting = [
				named_properties.father,
				named_properties.mother,
				named_properties.sibling,
			];
			return inheriting.includes(this.add_item.relation);
		},
		// Parse the typed name, append the main item's surname, then detect the
		// gender — e.g. main "Heinrich von Decker" turns "Johann Friedrich Wilhelm
		// (* 1789)" into "Johann Friedrich Wilhelm von Decker". The surname is
		// added before gender detection so the API call sees the combined name,
		// and is skipped if the name already contains it. Best-effort (see
		// surnameFromLabel).
		async addSurnameFromMain() {
			this.cleanNewPersonName();
			const surname = surnameFromLabel(this.labelFor(this.main_q));
			const name = (this.add_item.name ?? '').trim();
			if (isNonEmptyString(surname) && !nameHasSurname(name, surname)) {
				this.add_item.name = isNonEmptyString(name) ? `${name} ${surname}` : surname;
			}
			await this.detectNewPersonGender();
		},
		async guessGenderFromName() {
			// The draft may be reset while the lookup runs; apply the answer only
			// to the draft it was asked for.
			const draft = this.add_item;
			const fields = document.getElementById('gender_fields');
			fields?.classList.add('gender-disabled');
			try {
				const timeout = new Promise((_, reject) =>
					setTimeout(() => reject(Object.assign(new Error('timeout'), { isTimeout: true })), 5000)
				);
				const json = await Promise.race([
					apiJson(NAME_GENDER_API_BASE + encodeURIComponent(draft.name)),
					timeout,
				]);
				const gender = json
					.filter((c) => c.mainsnak.property === named_properties.gender)
					.map((c) => c.mainsnak.datavalue.value.id);
				if (gender.length === 1 && this.add_item === draft) draft.gender = gender[0];
			} catch (e) {
				if (!e.isTimeout) this.addError(`Gender guess failed: ${e.message || e}`);
			} finally {
				fields?.classList.remove('gender-disabled');
			}
		},

		createItemFromInterface() {
			if (this.pending_confirm) return;
			const new_item = { ...this.add_item };
			const main_q = this.main_q;
			const source_url = appState.source_url;
			// The button is disabled for an incomplete draft; returning quietly also
			// swallows a second click that lands before Vue re-renders the form.
			if (!isNonEmptyString(new_item.name?.trim()) || !isNonEmptyString(new_item.gender?.trim())) return;
			// The Born/Died fields hold free-form text; normalise them to the
			// ISO-ish form newClaimDate expects so an unparseable value can't
			// reach it and yield an undefined claim (issue #76). Doing it here
			// also makes the pre-creation duplicate check see real years.
			for (const field of ['born', 'died']) {
				const raw = new_item[field];
				if (!isNonEmptyString(raw)) continue;
				const parsed = normalizeDateInput(raw);
				if (!isNonEmptyString(parsed))
					this.addError(`Could not parse ${field} date "${raw}"; it will be omitted.`);
				new_item[field] = parsed;
			}
			this.confirmIfWarnings(this.newItemWarnings(new_item, main_q),
				() => this.startCreation(new_item, main_q, source_url));
		},
		// Validate the relation a brand-new person would have to the main item,
		// using the typed birth/death dates (the item has no QID yet).
		newItemWarnings(new_item, main_q = this.main_q) {
			if (!isNonEmptyString(new_item.relation) || !isNonEmptyString(main_q)) return [];
			const dateOverrides = {
				[PENDING_NEW_Q]: {
					birth: timeValueToInterval(wikidataTimeValue(normalizeDateInput(new_item.born))),
					death: timeValueToInterval(wikidataTimeValue(normalizeDateInput(new_item.died))),
				},
			};
			const labels = { [PENDING_NEW_Q]: isNonEmptyString(new_item.name) ? new_item.name : 'the new person' };
			return this.warningMessages(
				[{ from: PENDING_NEW_Q, prop: new_item.relation, to: main_q }],
				dateOverrides, labels);
		},
		useExistingItem(job) {
			if (!isNonEmptyString(job?.duplicate_q) || !isNonEmptyString(job.item.relation)) return;
			bus.$emit('prefill-add-relation', {
				q: job.main_q,
				prop: job.item.relation,
				target_q: job.duplicate_q,
			});
			this.dismissCreationJob(job.id);
		},
		// The duplicate check flagged an existing item, but the user judged it
		// a wrong match — create the new item regardless, skipping the check.
		createPendingItemAnyway(job) {
			if (!job || job.status === 'creating') return;
			job.duplicate_q = '';
			this.createItem(job);
		},
		// Start a creation job and free the form for the next person immediately.
		// All data the job needs is snapshotted, so later typing cannot change it.
		startCreation(new_item, job_main_q = this.main_q, source_url = appState.source_url) {
			this.resetAddItem();
			const job = {
				id: ++this.creation_seq,
				main_q: job_main_q,
				item: new_item,
				source_url,
				status: 'checking',
				duplicate_q: '',
				error: '',
			};
			this.creation_jobs.push(job);
			this.checkBeforeItemCreation(job);
		},
		dismissCreationJob(id) {
			this.creation_jobs = this.creation_jobs.filter((j) => j.id !== id);
		},
		// Put a failed job's data back into the form, but never overwrite input
		// the user has started on since.
		restoreDraft(job) {
			if (!job || !this.isAddItemEmpty()) return;
			this.add_item = { ...job.item };
			this.dismissCreationJob(job.id);
		},
		async checkBeforeItemCreation(job) {
			const new_item = job.item;
			try {
				if (!isNonEmptyString(new_item.born) && !isNonEmptyString(new_item.died)) {
					await this.createItem(job);
					return;
				}
				const titles = await searchHumansMatching(new_item.name);
				if (titles.length === 0) { await this.createItem(job); return; }
				const wd = getWd();
				await wd.getItemBatch(titles);
				const yearOf = (s) => (s ?? '').replace(/-.*$/, '');
				const target_born = yearOf(new_item.born);
				const target_died = yearOf(new_item.died);
				const yearsForProp = (item, prop) =>
					item.getClaimsForProperty(prop)
						.filter((d) => d.mainsnak.datavalue !== undefined)
						.map((d) => d.mainsnak.datavalue.value.time.replace(/^\D*(\d+).*$/, '$1'));
				const found = titles.find((q) => {
					const i = wd.getItem(q);
					if (!i) return false;
					if (target_born && !yearsForProp(i, BIRTH_PROP).includes(target_born)) return false;
					if (target_died && !yearsForProp(i, DEATH_PROP).includes(target_died)) return false;
					return true;
				});
				if (!found) { await this.createItem(job); return; }
				await wd.getItemBatch([found]);
				job.duplicate_q = found;
				job.status = 'duplicate';
			} catch (e) {
				this.addError(`Pre-creation check failed: ${e.message || e}`);
				await this.createItem(job);
			}
		},
		async createItem(job) {
			// Set the status before the first await so a second click on "create
			// anyway" cannot start a second creation of the same person.
			if (job.status === 'creating') return;
			job.status = 'creating';
			const new_item = job.item;
			if (!isNonEmptyString(new_item.name?.trim())) { this.reportCreationError(job, 'Name is required'); return; }
			if (!isNonEmptyString(new_item.gender?.trim())) { this.reportCreationError(job, 'Gender is required'); return; }

			let logged_in = false;
			try {
				logged_in = await this.confirmLogin();
			} catch (e) {
				this.addError(`Could not check login: ${e.message || e}`);
			}
			if (!logged_in) {
				job.status = 'error';
				job.error = 'not logged in';
				return;
			}

			let d;
			try {
				const reference = (s) => this.addReferenceToStatement(s, job.source_url);
				const data = { labels: { mul: { language: 'mul', value: new_item.name } }, claims: [] };
				data.claims.push(reference(state.widar.newClaimEntity(INSTANCE_OF, HUMAN)));
				if (isNonEmptyString(new_item.gender))
					data.claims.push(reference(state.widar.newClaimEntity(named_properties.gender, new_item.gender)));
				if (isNonEmptyString(new_item.relation)) {
					let relationClaim = state.widar.newClaimEntity(new_item.relation, job.main_q);
					if (new_item.relation === named_properties.spouse)
						relationClaim = this.addMarriageQualifier(relationClaim, new_item.married);
					data.claims.push(reference(relationClaim));
				}
				if (isNonEmptyString(new_item.born))
					data.claims.push(reference(state.widar.newClaimDate(BIRTH_PROP, new_item.born)));
				if (isNonEmptyString(new_item.died))
					data.claims.push(reference(state.widar.newClaimDate(DEATH_PROP, new_item.died)));

				const params = {
					action: 'generic',
					summary: NEW_ITEM_SUMMARY,
					tool_hashtag: 'relator2',
					json: JSON.stringify({ action: 'wbeditentity', new: 'item', data: JSON.stringify(data) }),
				};
				d = await widarRequest((cb) => state.widar.run(params, cb));
			} catch (e) {
				this.reportCreationError(job, `Could not start item creation: ${e.message || e}`);
				return;
			}
			if (d?.error !== 'OK') {
				this.reportEditError('Item creation failed', d?.error ?? 'unknown error');
				this.reportCreationError(job, d?.error ?? 'unknown error');
				return;
			}

			try {
				const q = d.res.entity.id;
				this.updateItemRaw(q, d.res.entity);
				if (job.main_q === appState.main_q) {
					numericSortItems(appState.items);
					addItem(q);
					bus.$emit('refresh-view');
				}
				this.dismissCreationJob(job.id);
				// When a marriage date was supplied for a spouse, write the
				// mirror P26 statement on the main item now (with the same P580
				// qualifier) instead of leaving it to a suggestion.
				const addMarriageMirror = new_item.relation === named_properties.spouse
					&& isNonEmptyString(new_item.married);
				if (addMarriageMirror) {
					this.persistEditsFor(job.main_q, [{
						from: job.main_q,
						prop: named_properties.spouse,
						to: q,
						married: new_item.married,
					}], job.source_url);
				}
			} catch (e) {
				this.addError(`Item creation post-processing failed: ${e.message || e}`);
				this.reportCreationError(job, `Post-processing failed: ${e.message || e}`);
			}
		},
		reportCreationError(job, message) {
			job.status = 'error';
			job.error = message || 'unknown error';
		},

		labelFor(q) {
			return labelForProperty(q, getWd(), state.tt?.language);
		},
	},
	template: `
<div>
    <div v-if="pending_confirm" class="relator2-confirm-backdrop" @click.self="cancelPendingSave">
        <div class="relator2-confirm card" role="alertdialog" aria-modal="true" aria-labelledby="relator2-confirm-title">
            <div class="card-header d-flex align-items-center gap-2">
                <span aria-hidden="true">⚠️</span>
                <strong id="relator2-confirm-title">This looks logically impossible</strong>
            </div>
            <div class="card-body">
                <ul class="mb-3 ps-3">
                    <li v-for="(msg, idx) in pending_confirm.warnings" :key="'warn_'+idx">{{msg}}</li>
                </ul>
                <div class="d-flex justify-content-end gap-2">
                    <button type="button" class="btn btn-sm btn-outline-secondary" @click.prevent="cancelPendingSave">Cancel</button>
                    <button type="button" class="btn btn-sm btn-danger" @click.prevent="confirmPendingSave">Save anyway</button>
                </div>
            </div>
        </div>
    </div>
    <div class="saving-toast-container" aria-live="polite" aria-atomic="true">
        <div v-if="activeJobCount() > 0" class="toast show align-items-center saving-toast" role="status">
            <div class="d-flex">
                <div class="toast-body d-flex align-items-center">
                    <span class="spinner-border spinner-border-sm me-2" aria-hidden="true"></span>
                    <span>Saving edits&hellip;<template v-if="activeJobCount() > 1"> ({{activeJobCount()}})</template></span>
                </div>
            </div>
        </div>
    </div>

    <div class="relator2-header">
        <div v-if="!notice_dismissed" class="relator2-notice" role="note">
            <span class="relator2-notice-icon" aria-hidden="true">💡</span>
            <span class="relator2-notice-text">
                Tip: Relator&nbsp;2 can add a link to your Wikipedia sidebar, show people in
                split views, and let you drag &amp; drop cards to connect them.
                <a :href="help_url" target="_blank" rel="noopener">Learn more</a>
            </span>
            <button type="button" class="relator2-notice-close" aria-label="Dismiss" @click.prevent="dismissNotice">&times;</button>
        </div>
        <div v-if="login_required" class="relator2-banner">
            <div class="alert alert-warning mb-1" role="alert">
                You are not logged in, so no edits can be saved.
                <a class="alert-link" :href="login_url" target="_blank" rel="noopener">Log in</a>,
                then <a class="alert-link" href="#" @click.prevent="recheckLogin">check again</a>.
            </div>
        </div>
        <div v-if="errors.length > 0" class="relator2-banner">
            <div v-for="(msg, idx) in errors" :key="'err_'+idx"
                class="alert alert-danger alert-dismissible mb-1" role="alert">
                <span class="error-text">{{msg}}</span>
                <button type="button" class="btn-close" aria-label="Close" @click.prevent="dismissError(idx)"></button>
            </div>
            <div v-if="errors.length > 1" class="text-end">
                <button type="button" class="btn btn-sm btn-outline-secondary" tt="clear_all" @click.prevent="clearErrors"></button>
            </div>
        </div>

        <tool-navbar>
            <template slot="help">
                <a class="help-button" target="_blank" :href="help_url" tt_title="help" aria-label="Help">
                    <svg xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 16 16" aria-hidden="true">
                        <path d="M8 15A7 7 0 1 1 8 1a7 7 0 0 1 0 14m0 1A8 8 0 1 0 8 0a8 8 0 0 0 0 16"/>
                        <path d="M5.255 5.786a.237.237 0 0 0 .241.247h.825c.138 0 .248-.113.266-.25.09-.656.54-1.134 1.342-1.134.686 0 1.314.343 1.314 1.168 0 .635-.374.927-.965 1.371-.673.489-1.206 1.06-1.168 1.987l.003.217a.25.25 0 0 0 .25.246h.811a.25.25 0 0 0 .25-.25v-.105c0-.718.273-.927 1.01-1.486.609-.463 1.244-.977 1.244-2.056 0-1.511-1.276-2.241-2.673-2.241-1.267 0-2.655.59-2.75 2.286zm1.557 5.763c0 .533.425.927 1.01.927.609 0 1.028-.394 1.028-.927 0-.552-.42-.94-1.029-.94-.584 0-1.009.388-1.009.94z"/>
                    </svg>
                </a>
            </template>
            <template slot="actions">
                <select class="form-select form-select-sm w-auto layout-select" title="Layout (press L to cycle)"
                    :value="layout" @change="setLayout($event.target.value)">
                    <option value="v2">Tree</option>
                    <option value="columns">Columns</option>
                    <option value="graph">Diagram</option>
                </select>
                <widar></widar>
                <span v-if="loading" class="random-button random-button-loading" role="status" tt_title="loading">
                    <span class="spinner-border spinner-border-sm" aria-hidden="true"></span>
                    <span class="visually-hidden" tt="loading"></span>
                </span>
                <router-link v-else to="/random" tt_title="load_random_person_with_child">
                    <img class="random-button" src="https://upload.wikimedia.org/wikipedia/commons/thumb/7/70/Random_font_awesome.svg/120px-Random_font_awesome.svg.png" alt="random" />
                </router-link>
            </template>
        </tool-navbar>

        <div v-if="!loading && main_q" class="add-item-form">
            <div class="add-item-fields">
                <div class="d-flex flex-wrap align-items-center gap-2">
                    <span tt="main_item_is" :tt1="labelFor(main_q)"></span>
                    <div class="d-flex flex-wrap align-items-center gap-3 mt-2" style="white-space: nowrap">
	                    <select class="form-select form-select-sm w-auto" v-model="add_item.relation">
	                        <option v-for="prop in relation_properties" v-if="isValidMainProp(prop)" :key="prop" :value="prop">{{labelFor(prop)}}</option>
	                    </select>
	                    <span tt="of"></span>
                    </div>
                    <input type="text" class="form-control form-control-sm add-item-name" tt_placeholder="ph_name" v-model="add_item.name" />
                </div>
                <div class="d-flex flex-wrap align-items-center gap-3 mt-2">
                    <span class="add-item-dates">
                        Born <input type="text" class="form-control form-control-sm d-inline-block w-auto" v-model="add_item.born" tt_placeholder="ph_date" />
                        Died <input type="text" class="form-control form-control-sm d-inline-block w-auto" v-model="add_item.died" tt_placeholder="ph_date" />
                    </span>
                    <span id="gender_fields" class="add-item-gender">
                        <label class="me-2"><input type="radio" v-model="add_item.gender" :value="named_items.male" /> <span tt="male"></span></label>
                        <label><input type="radio" v-model="add_item.gender" :value="named_items.female" /> <span tt="female"></span></label>
                    </span>
                </div>
                <div v-if="is_spouse_relation" class="d-flex flex-wrap align-items-center gap-2 mt-2 add-item-married">
                    <span tt="married"></span>
                    <input type="text" class="form-control form-control-sm d-inline-block w-auto"
                        v-model="add_item.married" tt_placeholder="ph_date" />
                </div>
                <div class="d-flex flex-wrap align-items-center gap-2 mt-2">
                    <div class="btn-group" role="group">
                        <button class="btn btn-sm btn-outline-secondary" tt="parse" @click.prevent="parseNewPersonName"></button>
                        <button v-if="showAddSurnameButton()" class="btn btn-sm btn-outline-secondary"
                            tt_title="parse_plus" @click.prevent="addSurnameFromMain">+</button>
                    </div>
                    <button class="btn btn-sm btn-outline-success" tt="create_new_item"
                        @click.prevent="createItemFromInterface"
                        :disabled="!canCreateNewItem()"></button>
                    <button class="btn btn-sm btn-outline-secondary" tt="clear"
                        @click.prevent="resetAddItem"></button>
                    <button v-if="showAddCheckedButton()" class="btn btn-outline-primary" tt="add_checked"
                        @click.prevent="onAddChecked" :disabled="claims_in_flight > 0"></button>
                </div>
                <div class="d-flex align-items-center mt-2 gap-2 source-url-row">
                    <span tt="source_url" class="source-url-label"></span>
                    <input type="text" class="form-control form-control-sm flex-grow-1 source-url-input" v-model="source_url" tt_placeholder="ph_source_url" />
                </div>
                <div v-for="job in creation_jobs" :key="'job_'+job.id" class="mt-2">
                    <div v-if="job.status === 'duplicate'"
                        class="alert alert-warning alert-dismissible mb-1 d-flex flex-wrap align-items-center gap-2" role="alert">
                        <span tt="item_exists_as"></span>
                        <wd-link :item="job.duplicate_q" :wd="wd"></wd-link>
                        <small>[{{job.duplicate_q}}]</small>
                        <button type="button" class="btn btn-sm btn-outline-primary" tt="add_as_relation"
                            @click.prevent="useExistingItem(job)"></button>
                        <button type="button" class="btn btn-sm btn-outline-success" tt="create_new_item"
                            @click.prevent="createPendingItemAnyway(job)"></button>
                        <button v-if="isAddItemEmpty()" type="button" class="btn btn-sm btn-outline-secondary"
                            @click.prevent="restoreDraft(job)">Restore draft</button>
                        <button type="button" class="btn-close" aria-label="Close"
                            @click.prevent="dismissCreationJob(job.id)"></button>
                    </div>
                    <div v-else-if="job.status === 'error'"
                        class="alert alert-danger alert-dismissible mb-1 d-flex flex-wrap align-items-center gap-2" role="alert">
                        <span>Could not create <strong>{{job.item.name}}</strong>: {{job.error}}</span>
                        <button v-if="isAddItemEmpty()" type="button" class="btn btn-sm btn-outline-secondary"
                            @click.prevent="restoreDraft(job)">Restore draft</button>
                        <button type="button" class="btn-close" aria-label="Close"
                            @click.prevent="dismissCreationJob(job.id)"></button>
                    </div>
                    <div v-else class="alert alert-secondary mb-1 d-flex align-items-center gap-2" role="status">
                        <span class="spinner-border spinner-border-sm" aria-hidden="true"></span>
                        <span>Creating <strong>{{job.item.name}}</strong>&hellip;</span>
                    </div>
                </div>
            </div>
        </div>
    </div>

    <div v-if="loading" class="d-flex align-items-center gap-2 m-3" role="status">
        <span class="spinner-border spinner-border-sm" aria-hidden="true"></span>
        <span tt="loading"></span>
    </div>
    <div v-else-if="!main_q" class="m-3">
        <div v-if="invalid_q !== ''" class="alert alert-danger" role="alert">
            Invalid item ID "<strong>{{invalid_q}}</strong>". Please enter a valid Q number (e.g. Q42).
        </div>
        <div class="mb-3 d-flex flex-wrap align-items-center gap-2">
            <span tt="enter_q"></span>
            <input type="text" class="form-control form-control-sm w-auto" v-model="new_q"
                tt_placeholder="ph_enter_q" @keyup.enter="openQ" />
            <button class="btn btn-sm btn-outline-success" tt="open_q"
                @click.prevent="openQ" :disabled="new_q.trim() === ''"></button>
        </div>
        <div>
            <router-link class="btn btn-sm btn-outline-success" to="/random"
                tt="load_random_person_with_child"></router-link>
        </div>
    </div>
    <div v-else class="relator2-body">
        <div v-if="layout === 'classic'">
            <div class="people-row" v-if="ancestors().length > 0">
                <people :items="ancestors()" :suggestions="suggestions" :is_busy="is_busy"
                    @add-suggestions="addSuggestions" @change-main-q="changeMainQ" @select-suggestion="selectSuggestion" @set-gender="setGender" @set-description="saveDescription"></people>
            </div>
            <div class="people-row">
                <people :items="spouses()" :suggestions="suggestions" :is_busy="is_busy"
                    @add-suggestions="addSuggestions" @change-main-q="changeMainQ" @select-suggestion="selectSuggestion" @set-gender="setGender" @set-description="saveDescription"></people>
                <people :items="[main_q]" :suggestions="suggestions" :is_busy="is_busy"
                    @add-suggestions="addSuggestions" @change-main-q="changeMainQ" @select-suggestion="selectSuggestion" @set-gender="setGender" @set-description="saveDescription"></people>
                <people :items="relatives()" :suggestions="suggestions" :is_busy="is_busy"
                    @add-suggestions="addSuggestions" @change-main-q="changeMainQ" @select-suggestion="selectSuggestion" @set-gender="setGender" @set-description="saveDescription"></people>
            </div>
            <div class="people-row" v-if="descendants().length > 0">
                <people :items="descendants()" :suggestions="suggestions" :is_busy="is_busy"
                    @add-suggestions="addSuggestions" @change-main-q="changeMainQ" @select-suggestion="selectSuggestion" @set-gender="setGender" @set-description="saveDescription"></people>
            </div>
        </div>
        <div v-else-if="layout === 'v2'">
            <div class="people-row" v-for="(row, idx) in render_rows" :key="'row_'+idx">
                <people :items="row" :suggestions="suggestions" :is_busy="is_busy"
                    @add-suggestions="addSuggestions" @change-main-q="changeMainQ" @select-suggestion="selectSuggestion" @set-gender="setGender" @set-description="saveDescription"></people>
            </div>
        </div>
        <div v-else-if="layout === 'columns'" class="people-columns">
            <div class="people-column" v-for="(row, idx) in render_rows" :key="'col_'+idx">
                <people :items="row" :suggestions="suggestions" :is_busy="is_busy"
                    @add-suggestions="addSuggestions" @change-main-q="changeMainQ" @select-suggestion="selectSuggestion" @set-gender="setGender" @set-description="saveDescription"></people>
            </div>
        </div>
        <div v-else-if="layout === 'graph'">
            <graph-view :suggestions="suggestions" :is_busy="is_busy"
                @add-suggestions="addSuggestions" @change-main-q="changeMainQ" @select-suggestion="selectSuggestion" @set-gender="setGender" @set-description="saveDescription"></graph-view>
        </div>
        <div v-else>Unknown layout "{{layout}}"</div>
    </div>
</div>`,
});

export default MainPage;
