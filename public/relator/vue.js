'use strict';

let router ;
let app ;
let wd = new WikiData() ;

let api = './api.php' ;
let relation_properties = ['P22','P25','P3448','P26','P3373','P1038','P451','P40'] ;
let mirror_properties = {
	'P22':['P40'],
	'P25':['P40'],
//	'P3448':[],
	'P26':['P26'],
	'P3373':['P3373'],
	'P1038':['P1038'],
	'P451':['P451'],
	'P40':['P22','P25'] // father,mother
} ;

$(document).ready ( function () {
	Promise.all ( [
		vue_components.loadComponents ( ['widar','wd-date','wd-link','tool-translate','tool-navbar','main-page.html','row-item.html'] ) ,
		new Promise(function(resolve, reject) { wd.getItemBatch ( relation_properties , resolve ) } )
	] )	.then ( () => {
		widar_api_url = './api.php' ;
			const routes = [
			  { path: '/', component: MainPage , props:true },
			  { path: '/:main_q', component: MainPage , props:true },
			] ;
			router = new VueRouter({routes}) ;
			app = new Vue ( { router } ) .$mount('#app') ;
		} ) ;
} ) ;
