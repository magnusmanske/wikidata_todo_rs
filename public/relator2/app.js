import { setToolName, setWd, setWidarApiUrl } from '../resources/vue_es6/state.js';
import { initToolTranslate } from '../resources/vue_es6/tool-translate.js';
import ToolNavbar from './vue_components/tool-navbar.js';
import Widar from '../resources/vue_es6/widar.js';
import WdLink from '../resources/vue_es6/wd-link.js';
import WdDate from '../resources/vue_es6/wd-date.js';

import { getWd } from './app-state.js';
import { relation_properties } from './relations.js';
import { fallbackDropPropertyIds } from './drop_rules.js';
import { loadRejected } from './rejection_store.js';
import MainPage from './vue_components/main-page.js';

Vue.component('tool-navbar', ToolNavbar);
Vue.component('widar', Widar);
Vue.component('wd-link', WdLink);
Vue.component('wd-date', WdDate);

const wd = getWd();
setToolName('relator2');
setWd(wd);
setWidarApiUrl('./api.php');

await initToolTranslate('relator');
loadRejected();
await wd.getItemBatch([...new Set([...relation_properties, ...fallbackDropPropertyIds()])]);

const router = new VueRouter({
    routes: [
        { path: '/',            component: MainPage, props: true },
        { path: '/:initial_q',  component: MainPage, props: true },
    ],
});

new Vue({ router }).$mount('#app');
