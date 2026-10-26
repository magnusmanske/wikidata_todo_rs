import { setToolName, setWd, setWidarApiUrl } from '../resources/vue_es6/state.js';
import { initToolTranslate } from '../resources/vue_es6/tool-translate.js';
import ToolNavbar from '../resources/vue_es6/tool-navbar.js';
import Widar from '../resources/vue_es6/widar.js';
import WdLink from '../resources/vue_es6/wd-link.js';

import { bus, getWd } from './app-state.js';
import MainPage from './vue_components/main-page.js';
import ArticlePage from './vue_components/article-page.js';
import ListPage from './vue_components/list-page.js';
import StatsPage from './vue_components/stats-page.js';
import CategoriesPage from './vue_components/categories-page.js';

Vue.component('tool-navbar', ToolNavbar);
Vue.component('widar', Widar);
Vue.component('wd-link', WdLink);

setToolName('duplicity');
setWd(getWd());
setWidarApiUrl('./api.php');

await initToolTranslate('duplicity');

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
