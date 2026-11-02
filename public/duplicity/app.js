import { setToolName, setWd, setWidarApiUrl, state } from '../resources/vue_es6/state.js';
import { initToolTranslate } from '../resources/vue_es6/tool-translate.js';
import Widar from '../resources/vue_es6/widar.js';
import WdLink from '../resources/vue_es6/wd-link.js';

import { bus, getWd } from './app-state.js';
import MainPage from './vue_components/main-page.js';
import ArticlePage from './vue_components/article-page.js';
import ListPage from './vue_components/list-page.js';
import StatsPage from './vue_components/stats-page.js';
import CategoriesPage from './vue_components/categories-page.js';

Vue.component('widar', Widar);
Vue.component('wd-link', WdLink);

setToolName('duplicity');
setWd(getWd());
setWidarApiUrl('./api.php');

const tt = await initToolTranslate('duplicity', {
    // Vue does not re-render on a language change, so re-translate the DOM.
    onLanguageChange: () => state.tt?.updateInterface(document.body),
});

const router = new VueRouter({
    routes: [
        { path: '/',                           component: MainPage,       props: true },
        { path: '/article/:wiki',              component: ArticlePage,    props: true },
        { path: '/article/:wiki/:fixed_page',  component: ArticlePage,    props: true },
        { path: '/list/:wiki',                 component: ListPage,       props: true },
        { path: '/list/:wiki/:category',       component: ListPage,       props: true },
        { path: '/stats/:wiki',                component: StatsPage,      props: true },
        { path: '/categories/:wiki',           component: CategoriesPage, props: true },
    ],
});

new Vue({
    router,
    data: { errors: [] },
    computed: {
        userName() { return state.widar?.userinfo?.name ?? ''; },
        userUrl() {
            return `https://www.wikidata.org/wiki/User:${encodeURIComponent(this.userName.replace(/ /g, '_'))}`;
        },
    },
    created() {
        bus.$on('error', this.onError);
    },
    beforeDestroy() {
        bus.$off('error', this.onError);
    },
    methods: {
        onError(msg) { this.errors.push(String(msg ?? '')); },
        dismissError(idx) { this.errors.splice(idx, 1); },
    },
}).$mount('#app');

// After mounting, so that the listener on the language select survives Vue's
// compilation of the page's markup.
tt.addILdropdown('#tooltranslate_wrapper');

// A click anywhere else closes an open toolbar dropdown.
document.addEventListener('click', (event) => {
    for (const dropdown of document.querySelectorAll('details.dropdown[open]')) {
        if (!dropdown.contains(event.target)) dropdown.open = false;
    }
});
